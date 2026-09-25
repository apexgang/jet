//! Harness extensions (wave 3.2 §4.4, slice 4): each Craft's native
//! catalog, inspection of one entry, and reviewed extension changes.
//!
//! The Craft's native metadata never crosses whole. The webview receives
//! bounded identifiers and opaque tokens: an entry token names one catalog
//! entry and an inspection ID names one exact inspection, both bound to the
//! Plane that issued them. The confirmation `jetd` revalidates is built
//! natively from the stored inspection, so the webview never sends a path,
//! an extension identifier or catalog JSON back.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use jet_protocol::{
    ExtensionAction, ExtensionCatalog, ExtensionChangeState, ExtensionConfirmation, ExtensionScope,
    ExtensionTrust,
};
use serde::Serialize;
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use super::{
    agents::{bounded_label, harness_display_name, validate_craft_id},
    errors::PublicError,
    planes::{self, PlaneId},
    settings::{plane_client, PlaneKey, ReviewSubject, SettingsAction, SettingsReviewView},
    JetBridge,
};

/// A canonical Plane handle is `local` or a 36-byte UUID.
const MAX_PLANE_ID_BYTES: usize = 36;
/// `jet-core/src/extension/work.rs`: the daemon refuses larger metadata.
const MAX_NATIVE_METADATA_BYTES: usize = 64 * 1024;
const MAX_STANDALONE: usize = 512;
const MAX_PLUGINS: usize = 256;
const MAX_STANDALONE_ID_BYTES: usize = 256;
/// Claude's plugin selector limit (`jet-craft-claude/src/extensions`).
const MAX_PLUGIN_ID_BYTES: usize = 160;
const MAX_FILES: usize = 256;
const MAX_FACT_BYTES: usize = 512;
const MAX_HASH_BYTES: usize = 128;
const CATALOG_CAPACITY: usize = 8;
const INSPECTION_CAPACITY: usize = 16;
const GRANT_LIFETIME: Duration = Duration::from_secs(10 * 60);
const CHANGE_CAPACITY: usize = 64;

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionCatalogView {
    craft_id: String,
    /// The Harness's product name, or its bounded identifier.
    harness: String,
    standalone: Vec<StandaloneEntryView>,
    plugins: Vec<PluginEntryView>,
    /// Changes queued from this app that have not reached a final state.
    changes: Vec<ExtensionChangeRefView>,
    /// More entries than this app shows.
    truncated: bool,
    issues: Vec<CatalogIssueView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StandaloneEntryView {
    entry_token: String,
    id: String,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginEntryView {
    entry_token: String,
    id: String,
    /// `None` when the catalog doesn't say.
    installed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionChangeRefView {
    change_id: String,
    extension_id: String,
    action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogIssueView {
    /// `plugins` or `catalog`.
    section: &'static str,
    error: PublicError,
}

/// One inspected entry as exact facts. Paths, source and publisher are
/// inert display text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionInspectionView {
    inspection_id: String,
    extension_id: String,
    #[serde(flatten)]
    facts: ExtensionFactsView,
    disabled: Option<bool>,
    /// Only where the Craft names them (Claude plugins).
    supported_actions: Option<Vec<&'static str>>,
    /// Every file the change touches is listed and shown exactly.
    reviewable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionFactsView {
    publisher: Option<String>,
    version: Option<String>,
    source: Option<String>,
    files: Vec<ExtensionFileView>,
    /// Every file the Craft listed, including any not shown.
    file_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionFileView {
    path: String,
    sha256: String,
}

/// What a reviewed extension change will do. Nothing in it is sent back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionReviewView {
    craft_id: String,
    harness: String,
    extension_id: String,
    action: &'static str,
    #[serde(flatten)]
    facts: ExtensionFactsView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionChangeView {
    change_id: String,
    craft_id: String,
    extension_id: String,
    action: &'static str,
    state: &'static str,
}

// ---------------------------------------------------------------------------
// Catalog projection
// ---------------------------------------------------------------------------

/// Which list an entry came from; only its identifier is kept natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Standalone,
    Plugin,
}

/// A projected catalog entry before it receives a token.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    id: String,
    /// Standalone: enabled. Plugin: installed, when known.
    state: Option<bool>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Projection {
    entries: Vec<Entry>,
    truncated: bool,
    plugins_unavailable: bool,
    unreadable: bool,
}

/// An identifier shown and later sent exactly as the Craft reported it:
/// non-empty, bounded and without control characters.
fn identifier(value: &Value, maximum_bytes: usize) -> Option<String> {
    let text = value.as_str()?;
    (!text.is_empty() && text.len() <= maximum_bytes && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

/// Projects bundled Craft catalog metadata to identifiers only. Paths,
/// native keys, explicit-source hints and permissions are dropped. An
/// unrecognized shape is reported, never guessed at.
fn project_catalog(metadata: &str) -> Projection {
    let mut projection = Projection::default();
    if metadata.len() > MAX_NATIVE_METADATA_BYTES {
        projection.unreadable = true;
        return projection;
    }
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(metadata) else {
        projection.unreadable = true;
        return projection;
    };
    let standalone = root.get("standalone");
    let plugins = root.get("plugins");
    if standalone.is_none() && plugins.is_none() {
        projection.unreadable = true;
        return projection;
    }
    let mut seen = HashSet::new();
    if let Some(standalone) = standalone {
        project_standalone(standalone, &mut projection, &mut seen);
    }
    if let Some(plugins) = plugins {
        project_plugins(plugins, &mut projection, &mut seen);
    }
    projection
}

fn project_standalone(value: &Value, projection: &mut Projection, seen: &mut HashSet<String>) {
    let Some(entries) = value.get("entries").and_then(Value::as_array) else {
        projection.unreadable = true;
        return;
    };
    let mut count = 0;
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(|id| identifier(id, MAX_STANDALONE_ID_BYTES));
        let enabled = entry.get("enabled").and_then(Value::as_bool);
        let (Some(id), Some(enabled)) = (id, enabled) else {
            projection.unreadable = true;
            continue;
        };
        if count == MAX_STANDALONE {
            projection.truncated = true;
            break;
        }
        // A skill kept both enabled and disabled is one entry to act on.
        if seen.insert(format!("s:{id}")) {
            count += 1;
            projection.entries.push(Entry {
                kind: EntryKind::Standalone,
                id,
                state: Some(enabled),
            });
        }
    }
}

/// Plugin lists by known Craft shape: Codex `{marketplaces[].plugins[]}`,
/// Claude an array or `{installed[], available[]}`.
fn project_plugins(value: &Value, projection: &mut Projection, seen: &mut HashSet<String>) {
    let groups: Vec<(&Vec<Value>, Option<bool>)> = match value {
        Value::Array(plugins) => vec![(plugins, None)],
        Value::Object(object)
            if object.get("plugin_catalog_unavailable") == Some(&Value::Bool(true)) =>
        {
            projection.plugins_unavailable = true;
            return;
        }
        Value::Object(object) if object.contains_key("marketplaces") => {
            let Some(marketplaces) = object.get("marketplaces").and_then(Value::as_array) else {
                projection.unreadable = true;
                return;
            };
            let mut groups = Vec::new();
            for marketplace in marketplaces {
                match marketplace.get("plugins").and_then(Value::as_array) {
                    Some(plugins) => groups.push((plugins, None)),
                    None => projection.unreadable = true,
                }
            }
            groups
        }
        Value::Object(object)
            if object.contains_key("installed") || object.contains_key("available") =>
        {
            let mut groups = Vec::new();
            for (key, installed) in [("installed", true), ("available", false)] {
                match object.get(key) {
                    None => {}
                    Some(Value::Array(plugins)) => groups.push((plugins, Some(installed))),
                    Some(_) => projection.unreadable = true,
                }
            }
            groups
        }
        _ => {
            projection.unreadable = true;
            return;
        }
    };
    let mut count = 0;
    for (plugins, listed) in groups {
        for plugin in plugins {
            // Only the user's own configuration is admitted (ExtensionScope::User).
            if plugin
                .get("scope")
                .and_then(Value::as_str)
                .is_some_and(|scope| scope != "user")
            {
                continue;
            }
            let Some(id) = plugin_id(plugin) else {
                projection.unreadable = true;
                continue;
            };
            if count == MAX_PLUGINS {
                projection.truncated = true;
                return;
            }
            if seen.insert(format!("p:{id}")) {
                count += 1;
                projection.entries.push(Entry {
                    kind: EntryKind::Plugin,
                    id,
                    state: listed.or_else(|| plugin.get("installed").and_then(Value::as_bool)),
                });
            }
        }
    }
}

/// A plugin's native identity: `id`, or Claude's `name@marketplace`.
fn plugin_id(plugin: &Value) -> Option<String> {
    if let Some(id) = plugin.get("id") {
        return identifier(id, MAX_PLUGIN_ID_BYTES);
    }
    let name = plugin.get("name")?.as_str()?;
    let marketplace = plugin.get("marketplace")?.as_str()?;
    identifier(
        &Value::String(format!("{name}@{marketplace}")),
        MAX_PLUGIN_ID_BYTES,
    )
}

// ---------------------------------------------------------------------------
// Inspection projection
// ---------------------------------------------------------------------------

/// Inert display text: bounded, no control characters. Anything else is
/// not shown (`None`), never replaced.
fn fact(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    (!text.is_empty() && text.len() <= MAX_FACT_BYTES && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

/// A publisher is text or an object with a `name`.
fn publisher(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Object(object) => fact(object.get("name")),
        other => fact(Some(other)),
    }
}

fn action_name(action: ExtensionAction) -> &'static str {
    match action {
        ExtensionAction::Install => "install",
        ExtensionAction::Update => "update",
        ExtensionAction::Disable => "disable",
        ExtensionAction::Remove => "remove",
    }
}

fn parse_action(value: &str) -> Result<ExtensionAction, PublicError> {
    match value {
        "install" => Ok(ExtensionAction::Install),
        "update" => Ok(ExtensionAction::Update),
        "disable" => Ok(ExtensionAction::Disable),
        "remove" => Ok(ExtensionAction::Remove),
        _ => Err(action_invalid()),
    }
}

/// What an inspection shows, and whether it can be confirmed: the Craft
/// listed at least one file and every file can be shown exactly
/// (`jet_craft_sdk::extension_review_complete` plus display bounds).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Inspected {
    facts: ExtensionFactsView,
    disabled: Option<bool>,
    supported_actions: Option<Vec<&'static str>>,
    reviewable: bool,
}

fn project_inspection(metadata: &str) -> Option<Inspected> {
    if metadata.len() > MAX_NATIVE_METADATA_BYTES {
        return None;
    }
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(metadata) else {
        return None;
    };
    let candidate = root.get("candidate").filter(|value| value.is_object());
    let from_candidate = |key: &str| candidate.and_then(|candidate| candidate.get(key));

    let listed: &[Value] = root
        .get("files")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let mut files = Vec::new();
    let mut exact = true;
    for file in listed.iter().take(MAX_FILES) {
        let path = fact(file.get("path"));
        let sha256 = file
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|hash| {
                !hash.is_empty()
                    && hash.len() <= MAX_HASH_BYTES
                    && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .map(str::to_owned);
        match (path, sha256) {
            (Some(path), Some(sha256)) => files.push(ExtensionFileView { path, sha256 }),
            _ => exact = false,
        }
    }
    let file_count = u32::try_from(listed.len()).unwrap_or(u32::MAX);
    let reviewable = !listed.is_empty() && exact && listed.len() <= MAX_FILES;

    let supported_actions =
        root.get("supported_actions")
            .and_then(Value::as_array)
            .map(|actions| {
                let mut known = Vec::new();
                for action in actions.iter().filter_map(Value::as_str) {
                    if let Ok(action) = parse_action(action) {
                        let name = action_name(action);
                        if !known.contains(&name) {
                            known.push(name);
                        }
                    }
                }
                known
            });

    Some(Inspected {
        facts: ExtensionFactsView {
            publisher: publisher(root.get("publisher"))
                .or_else(|| publisher(from_candidate("publisher"))),
            version: fact(root.get("version")).or_else(|| fact(from_candidate("version"))),
            source: fact(root.get("source"))
                .or_else(|| fact(from_candidate("path")))
                .or_else(|| {
                    fact(
                        root.get("details")
                            .and_then(|details| details.pointer("/plugin/summary/source/path")),
                    )
                }),
            files,
            file_count,
        },
        disabled: root.get("disabled").and_then(Value::as_bool),
        supported_actions,
        reviewable,
    })
}

fn harness_name(harness: &str) -> String {
    harness_display_name(harness)
        .map(str::to_owned)
        .unwrap_or_else(|| bounded_label(harness, 64, "Harness"))
}

// ---------------------------------------------------------------------------
// Native grants
// ---------------------------------------------------------------------------

/// Tokens, inspections and queued changes. None of the native values they
/// hold crosses to the webview.
#[derive(Default)]
pub(crate) struct ExtensionsState {
    catalogs: Mutex<Vec<CatalogGrant>>,
    inspections: Mutex<HashMap<Uuid, InspectionGrant>>,
    changes: Mutex<Vec<ChangeRecord>>,
}

struct CatalogGrant {
    plane: PlaneKey,
    craft_id: String,
    /// Entry token → exact extension identifier.
    entries: HashMap<Uuid, String>,
    created_at: Instant,
}

struct InspectionGrant {
    plane: PlaneKey,
    extension_id: String,
    /// The exact inspection the Craft returned; the confirmation carries it.
    catalog: ExtensionCatalog,
    harness: String,
    facts: ExtensionFactsView,
    reviewable: bool,
    created_at: Instant,
}

/// What an entry token resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryLookup {
    plane: PlaneKey,
    craft_id: String,
    extension_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangeRecord {
    plane: PlaneKey,
    change_id: Uuid,
    craft_id: String,
    extension_id: String,
    action: ExtensionAction,
    finished: bool,
}

fn live(created_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(created_at) < GRANT_LIFETIME
}

impl ExtensionsState {
    /// Issues fresh entry tokens for one catalog. A reload of the same
    /// Craft on the same Plane replaces its earlier tokens.
    fn grant_catalog(
        &self,
        plane: PlaneKey,
        craft_id: &str,
        ids: impl IntoIterator<Item = String>,
        now: Instant,
    ) -> Result<Vec<Uuid>, PublicError> {
        let mut catalogs = self.catalogs.lock().map_err(|_| PublicError::internal())?;
        catalogs.retain(|grant| {
            live(grant.created_at, now) && !(grant.plane == plane && grant.craft_id == craft_id)
        });
        while catalogs.len() >= CATALOG_CAPACITY {
            let Some(oldest) = catalogs
                .iter()
                .enumerate()
                .min_by_key(|(_, grant)| grant.created_at)
                .map(|(index, _)| index)
            else {
                break;
            };
            catalogs.remove(oldest);
        }
        let mut entries = HashMap::new();
        let mut tokens = Vec::new();
        for id in ids {
            let token = Uuid::new_v4();
            entries.insert(token, id);
            tokens.push(token);
        }
        catalogs.push(CatalogGrant {
            plane,
            craft_id: craft_id.to_owned(),
            entries,
            created_at: now,
        });
        Ok(tokens)
    }

    /// An entry token issued for `plane`. A token of another Plane is as
    /// unknown as an expired one.
    fn entry(&self, token: Uuid, plane: PlaneId, now: Instant) -> Result<EntryLookup, PublicError> {
        let catalogs = self.catalogs.lock().map_err(|_| PublicError::internal())?;
        catalogs
            .iter()
            .filter(|grant| grant.plane.plane == plane && live(grant.created_at, now))
            .find_map(|grant| {
                grant.entries.get(&token).map(|id| EntryLookup {
                    plane: grant.plane,
                    craft_id: grant.craft_id.clone(),
                    extension_id: id.clone(),
                })
            })
            .ok_or_else(inspection_expired)
    }

    fn grant_inspection(&self, grant: InspectionGrant, now: Instant) -> Result<Uuid, PublicError> {
        let mut inspections = self
            .inspections
            .lock()
            .map_err(|_| PublicError::internal())?;
        inspections.retain(|_, inspection| live(inspection.created_at, now));
        while inspections.len() >= INSPECTION_CAPACITY {
            let Some(oldest) = inspections
                .iter()
                .min_by_key(|(_, inspection)| inspection.created_at)
                .map(|(id, _)| *id)
            else {
                break;
            };
            inspections.remove(&oldest);
        }
        let id = Uuid::new_v4();
        inspections.insert(id, grant);
        Ok(id)
    }

    /// The confirmation for one inspection issued for `plane`, and the
    /// facts its review shows.
    fn confirmation(
        &self,
        id: Uuid,
        plane: PlaneId,
        action: ExtensionAction,
        now: Instant,
    ) -> Result<(PlaneKey, ExtensionConfirmation, ExtensionReviewView), PublicError> {
        let inspections = self
            .inspections
            .lock()
            .map_err(|_| PublicError::internal())?;
        let grant = inspections
            .get(&id)
            .filter(|grant| grant.plane.plane == plane && live(grant.created_at, now))
            .ok_or_else(inspection_expired)?;
        if !grant.reviewable {
            return Err(not_reviewable());
        }
        let review = ExtensionReviewView {
            craft_id: bounded_label(&grant.catalog.craft_id, 128, "Craft"),
            harness: grant.harness.clone(),
            extension_id: grant.extension_id.clone(),
            action: action_name(action),
            facts: grant.facts.clone(),
        };
        Ok((
            grant.plane,
            ExtensionConfirmation {
                catalog: grant.catalog.clone(),
                extension_id: grant.extension_id.clone(),
                action,
                scope: ExtensionScope::User,
                trust: ExtensionTrust::SameUserExecutable,
            },
            review,
        ))
    }

    /// Keeps a queued change so its status can be read after navigation.
    pub(crate) fn record_change(
        &self,
        plane: PlaneKey,
        change_id: Uuid,
        confirmation: &ExtensionConfirmation,
    ) {
        let Ok(mut changes) = self.changes.lock() else {
            return;
        };
        if changes.iter().any(|change| change.change_id == change_id) {
            return;
        }
        if changes.len() >= CHANGE_CAPACITY {
            // Oldest finished first, then oldest.
            let index = changes
                .iter()
                .position(|change| change.finished)
                .unwrap_or(0);
            changes.remove(index);
        }
        changes.push(ChangeRecord {
            plane,
            change_id,
            craft_id: confirmation.catalog.craft_id.clone(),
            extension_id: confirmation.extension_id.clone(),
            action: confirmation.action,
            finished: false,
        });
    }

    fn change(&self, change_id: Uuid, plane: PlaneId) -> Result<ChangeRecord, PublicError> {
        let changes = self.changes.lock().map_err(|_| PublicError::internal())?;
        changes
            .iter()
            .find(|change| change.change_id == change_id && change.plane.plane == plane)
            .cloned()
            .ok_or_else(change_unknown)
    }

    fn finish(&self, change_id: Uuid) {
        if let Ok(mut changes) = self.changes.lock() {
            if let Some(change) = changes
                .iter_mut()
                .find(|change| change.change_id == change_id)
            {
                change.finished = true;
            }
        }
    }

    /// Unfinished changes of one Craft on one Plane.
    fn pending(&self, plane: PlaneId, craft_id: &str) -> Vec<ExtensionChangeRefView> {
        let Ok(changes) = self.changes.lock() else {
            return Vec::new();
        };
        changes
            .iter()
            .filter(|change| {
                !change.finished && change.plane.plane == plane && change.craft_id == craft_id
            })
            .map(|change| ExtensionChangeRefView {
                change_id: change.change_id.to_string(),
                extension_id: change.extension_id.clone(),
                action: action_name(change.action),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn plane_handle(plane_id: &str) -> Result<PlaneId, PublicError> {
    if plane_id.len() > MAX_PLANE_ID_BYTES {
        return Err(planes::unknown_plane());
    }
    PlaneId::parse(plane_id)
}

fn catalog_view(
    bridge: &JetBridge,
    plane: PlaneKey,
    craft_id: &str,
    catalog: &ExtensionCatalog,
    now: Instant,
) -> Result<ExtensionCatalogView, PublicError> {
    let projection = project_catalog(&catalog.native_metadata);
    let tokens = bridge.extensions.grant_catalog(
        plane,
        craft_id,
        projection.entries.iter().map(|entry| entry.id.clone()),
        now,
    )?;
    let mut standalone = Vec::new();
    let mut plugins = Vec::new();
    for (entry, token) in projection.entries.into_iter().zip(tokens) {
        match entry.kind {
            EntryKind::Standalone => standalone.push(StandaloneEntryView {
                entry_token: token.to_string(),
                id: entry.id,
                enabled: entry.state.unwrap_or(false),
            }),
            EntryKind::Plugin => plugins.push(PluginEntryView {
                entry_token: token.to_string(),
                id: entry.id,
                installed: entry.state,
            }),
        }
    }
    let mut issues = Vec::new();
    if projection.plugins_unavailable {
        issues.push(CatalogIssueView {
            section: "plugins",
            error: PublicError::invalid_input(
                "extensions.plugin_catalog_unavailable",
                "Jet couldn't get this Harness's plugin list.",
            ),
        });
    }
    if projection.unreadable {
        issues.push(CatalogIssueView {
            section: "catalog",
            error: PublicError::invalid_input(
                "extensions.catalog_unreadable",
                "Some of this Harness's extensions can't be shown here.",
            ),
        });
    }
    Ok(ExtensionCatalogView {
        craft_id: craft_id.to_owned(),
        harness: harness_name(&catalog.harness),
        standalone,
        plugins,
        changes: bridge.extensions.pending(plane.plane, craft_id),
        truncated: projection.truncated,
        issues,
    })
}

#[tauri::command]
pub(crate) async fn load_extension_catalog(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    craft_id: String,
) -> Result<ExtensionCatalogView, PublicError> {
    load_extension_catalog_for(&bridge, &plane_id, &craft_id).await
}

pub(crate) async fn load_extension_catalog_for(
    bridge: &JetBridge,
    plane_id: &str,
    craft_id: &str,
) -> Result<ExtensionCatalogView, PublicError> {
    validate_craft_id(craft_id)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let catalog = connection
            .query(connection.extension_catalog(craft_id.to_owned()))
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        // The answer must be about the Craft that was asked.
        if catalog.craft_id != craft_id {
            return Err(PublicError::internal());
        }
        catalog_view(bridge, binding, craft_id, &catalog, Instant::now())
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn inspect_extension(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    entry_token: String,
) -> Result<ExtensionInspectionView, PublicError> {
    inspect_extension_for(&bridge, &plane_id, &entry_token).await
}

pub(crate) async fn inspect_extension_for(
    bridge: &JetBridge,
    plane_id: &str,
    entry_token: &str,
) -> Result<ExtensionInspectionView, PublicError> {
    let plane = plane_handle(plane_id)?;
    let token = Uuid::parse_str(entry_token).map_err(|_| inspection_expired())?;
    let entry = bridge.extensions.entry(token, plane, Instant::now())?;
    let binding = entry.plane;
    async {
        // The inspection goes only to the Plane that issued the token.
        let client = bridge.bound(&binding)?;
        // ASVS 4.3.2: the identifier comes from the native grant, never the webview.
        let connection = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let catalog = connection
            .query(connection.inspect_extension(entry.craft_id.clone(), entry.extension_id.clone()))
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        if catalog.craft_id != entry.craft_id {
            return Err(PublicError::internal());
        }
        let inspected =
            project_inspection(&catalog.native_metadata).ok_or_else(PublicError::internal)?;
        let harness = harness_name(&catalog.harness);
        let inspection_id = bridge.extensions.grant_inspection(
            InspectionGrant {
                plane: binding,
                extension_id: entry.extension_id.clone(),
                catalog,
                harness,
                facts: inspected.facts.clone(),
                reviewable: inspected.reviewable,
                created_at: Instant::now(),
            },
            Instant::now(),
        )?;
        Ok(ExtensionInspectionView {
            inspection_id: inspection_id.to_string(),
            extension_id: entry.extension_id,
            facts: inspected.facts,
            disabled: inspected.disabled,
            supported_actions: inspected.supported_actions,
            reviewable: inspected.reviewable,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn prepare_extension_change(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    inspection_id: String,
    action: String,
) -> Result<SettingsReviewView, PublicError> {
    prepare_extension_change_for(&bridge, &plane_id, &inspection_id, &action)
}

/// Admits one exact change of an inspected entry. No I/O: `jetd` and the
/// Craft revalidate the inspection and refuse what they don't allow.
pub(crate) fn prepare_extension_change_for(
    bridge: &JetBridge,
    plane_id: &str,
    inspection_id: &str,
    action: &str,
) -> Result<SettingsReviewView, PublicError> {
    let plane = plane_handle(plane_id)?;
    let action = parse_action(action)?;
    let id = Uuid::parse_str(inspection_id).map_err(|_| inspection_expired())?;
    let (binding, confirmation, preview) =
        bridge
            .extensions
            .confirmation(id, plane, action, Instant::now())?;
    let review_id = bridge.settings.admit(
        binding,
        SettingsAction::ChangeExtension { confirmation },
        None,
        Instant::now(),
    )?;
    Ok(SettingsReviewView::action(
        review_id,
        ReviewSubject::ChangeExtension { preview },
    ))
}

#[tauri::command]
pub(crate) async fn load_extension_change(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    change_id: String,
) -> Result<ExtensionChangeView, PublicError> {
    load_extension_change_for(&bridge, &plane_id, &change_id).await
}

pub(crate) async fn load_extension_change_for(
    bridge: &JetBridge,
    plane_id: &str,
    change_id: &str,
) -> Result<ExtensionChangeView, PublicError> {
    let plane = plane_handle(plane_id)?;
    let change_id = Uuid::parse_str(change_id).map_err(|_| change_unknown())?;
    let record = bridge.extensions.change(change_id, plane)?;
    let binding = record.plane;
    async {
        let client = bridge.bound(&binding)?;
        let connection = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let change = connection
            .query(connection.extension_change(change_id))
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        if change.change_id != change_id
            || change.craft_id != record.craft_id
            || change.extension_id != record.extension_id
            || change.action != record.action
        {
            return Err(PublicError::internal());
        }
        let state = match change.state {
            ExtensionChangeState::Staged => "staged",
            ExtensionChangeState::Applied => "applied",
            ExtensionChangeState::Refused => "refused",
            ExtensionChangeState::OutcomeUnknown => "outcome_unknown",
        };
        if change.state != ExtensionChangeState::Staged {
            bridge.extensions.finish(change_id);
        }
        Ok(ExtensionChangeView {
            change_id: change_id.to_string(),
            craft_id: bounded_label(&record.craft_id, 128, "Craft"),
            extension_id: record.extension_id,
            action: action_name(record.action),
            state,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

// ---------------------------------------------------------------------------
// Stable shell codes (§4.7)
// ---------------------------------------------------------------------------

fn inspection_expired() -> PublicError {
    PublicError::invalid_input(
        "extensions.inspection_expired",
        "This extension list is out of date. Check again.",
    )
}

fn not_reviewable() -> PublicError {
    PublicError::invalid_input(
        "extensions.not_reviewable",
        "Jet can't show what this change touches, so it can't be confirmed here.",
    )
}

fn action_invalid() -> PublicError {
    PublicError::invalid_input(
        "extensions.action_invalid",
        "Choose install, update, disable or remove.",
    )
}

fn change_unknown() -> PublicError {
    PublicError::invalid_input(
        "extensions.change_unknown",
        "Jet doesn't know that extension change on this Plane.",
    )
}

#[cfg(test)]
mod tests;
