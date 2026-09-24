//! Plane-addressed settings (wave 3.2 §4.3, §4.5). Every settings command
//! resolves its Plane here, before any I/O, and binds its snapshot and its
//! review to that Plane.
//!
//! `jetd` owns every policy. The shell only checks shape and size, places
//! rows from a static scope table, and refuses to act on a view it already
//! knows is outdated (the fresh-read guard). Writes stay last-writer-wins:
//! `SetSetting` carries no expected revision.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use jet_client::ClientError;
use jet_protocol::{
    AutoContinuePolicy, AutoContinueTarget, CapabilityObservation, CraftDisableMode,
    CraftInstallationConfirmation, CredentialSource, ExtensionConfirmation, PlaneStatus,
    RecoveryState, ResolvedSetting, SecurityState, SettingKey, SettingScope, SettingSelection,
    SettingSource, SettingValue,
};
use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, State};
use tokio::task::AbortHandle;
use uuid::Uuid;

use super::{
    agents::{self, AutoContinuePolicyView, CraftPreviewView},
    channels::parse_resume_cursor,
    client::{Connection, NativeUpdate, PlaneClient},
    errors::PublicError,
    extensions::ExtensionReviewView,
    planes,
    planes::{PlaneBinding, PlaneId},
    setup::project_name,
    JetBridge,
};

/// A canonical Plane handle is `local` or a 36-byte UUID.
const MAX_PLANE_ID_BYTES: usize = 36;
/// The longest Setting spelling is 30 bytes; anything longer is not a key.
const MAX_KEY_BYTES: usize = 64;
/// `jet-core/src/setting/mod.rs:33`.
const MAX_SETTING_TEXT_BYTES: usize = 2_048;
const SNAPSHOT_CAPACITY: usize = 32;
const SNAPSHOT_LIFETIME: Duration = Duration::from_secs(30 * 60);
const REVIEW_CAPACITY: usize = 128;
const REVIEW_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// A snapshot names each of the 24 keys once; more is not a snapshot.
const MAX_SNAPSHOT_SETTINGS: usize = 64;
const MAX_PROJECTS: usize = 256;
const MAX_BINDINGS: usize = 64;

/// The Plane a settings target, snapshot or review belongs to, and the Plane
/// identity known when it was resolved. Follow-up commands act only through
/// the Plane that issued them.
pub(crate) type PlaneKey = PlaneBinding;

/// Resolves a webview Plane handle to a registered Plane. An oversized,
/// non-canonical or unregistered handle is `plane.unknown`; nothing connects.
pub(crate) fn plane_client(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<(PlaneKey, PlaneClient), PublicError> {
    // ASVS 2.2.1: bounded before the registry sees it.
    if plane_id.len() > MAX_PLANE_ID_BYTES {
        return Err(planes::unknown_plane());
    }
    bridge.plane(Some(plane_id))
}

/// Parses a Plane handle without resolving it, for commands that act only
/// through a Plane stored natively with their snapshot or review.
fn plane_handle(plane_id: &str) -> Result<PlaneId, PublicError> {
    if plane_id.len() > MAX_PLANE_ID_BYTES {
        return Err(planes::unknown_plane());
    }
    PlaneId::parse(plane_id)
}

// ---------------------------------------------------------------------------
// Scope table and key spellings
// ---------------------------------------------------------------------------

/// The kind of scope a Setting may be stored at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Plane,
    Project,
    Conversation,
}

impl ScopeKind {
    fn of(scope: &SettingScope) -> Self {
        match scope {
            SettingScope::Plane => Self::Plane,
            SettingScope::Project { .. } => Self::Project,
            SettingScope::Conversation { .. } => Self::Conversation,
        }
    }
}

const PLANE_ONLY: &[ScopeKind] = &[ScopeKind::Plane];
const PROJECT_OR_CONVERSATION: &[ScopeKind] = &[ScopeKind::Project, ScopeKind::Conversation];
const EVERY_SCOPE: &[ScopeKind] = &[
    ScopeKind::Plane,
    ScopeKind::Project,
    ScopeKind::Conversation,
];

/// Where each Setting may be stored. Mirrors `jet-core/src/setting/key.rs`
/// `CATALOG`; the daemon's `setting.scope_unsupported` stays the authority
/// if this ever drifts. The snapshot is never used for placement: the core
/// resolves every key the minor names at every scope.
pub(crate) const SETTING_SCOPES: [(SettingKey, &[ScopeKind]); 24] = [
    (SettingKey::StorageDisposableMiB, PLANE_ONLY),
    (SettingKey::EnergyConcurrency, PLANE_ONLY),
    (SettingKey::EnergyLowPowerConcurrency, PLANE_ONLY),
    (SettingKey::EnergyConstrained, PLANE_ONLY),
    (SettingKey::EnergyForegroundOverride, PLANE_ONLY),
    (SettingKey::ArtifactMaxMiB, PLANE_ONLY),
    (SettingKey::ArtifactRunMiB, PLANE_ONLY),
    (SettingKey::UtilityAccountBinding, PLANE_ONLY),
    (SettingKey::UtilityContentConsent, PLANE_ONLY),
    (SettingKey::UtilityAutodeleteCompilation, PLANE_ONLY),
    (SettingKey::UtilityGitText, PLANE_ONLY),
    (SettingKey::UtilityAutomaticNaming, EVERY_SCOPE),
    (SettingKey::GitAutoCommit, PROJECT_OR_CONVERSATION),
    (SettingKey::GitAutoBranch, PROJECT_OR_CONVERSATION),
    (SettingKey::GitAutoPush, PROJECT_OR_CONVERSATION),
    (SettingKey::GitAutoDraftPullRequest, PROJECT_OR_CONVERSATION),
    (SettingKey::GitBranchPrefix, PROJECT_OR_CONVERSATION),
    (SettingKey::GitMessageInstructions, PLANE_ONLY),
    (SettingKey::SecurityAuditRetentionDays, PLANE_ONLY),
    (SettingKey::RetentionTrashGraceDays, PLANE_ONLY),
    (SettingKey::DeveloperMode, PLANE_ONLY),
    (SettingKey::AutomaticReview, PLANE_ONLY),
    (SettingKey::AutomaticReviewBinding, PLANE_ONLY),
    (SettingKey::AutomaticReviewConsent, PLANE_ONLY),
];

/// Whether `key` may be written at `scope` according to `SETTING_SCOPES`.
fn allowed(key: SettingKey, scope: &SettingScope) -> bool {
    let kind = ScopeKind::of(scope);
    SETTING_SCOPES
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .is_some_and(|(_, scopes)| scopes.contains(&kind))
}

/// The protocol spelling of a Setting. Exhaustive: a new key fails to build
/// until it is placed here (never `Debug`).
pub(crate) fn key_spelling(key: SettingKey) -> &'static str {
    match key {
        SettingKey::StorageDisposableMiB => "storage.disposable_mib",
        SettingKey::EnergyConcurrency => "energy.concurrency",
        SettingKey::EnergyLowPowerConcurrency => "energy.low_power_concurrency",
        SettingKey::EnergyConstrained => "energy.constrained",
        SettingKey::EnergyForegroundOverride => "energy.foreground_override",
        SettingKey::ArtifactMaxMiB => "artifact.max_mib",
        SettingKey::ArtifactRunMiB => "artifact.run_mib",
        SettingKey::UtilityAccountBinding => "utility.account_binding",
        SettingKey::UtilityContentConsent => "utility.content_consent",
        SettingKey::UtilityAutodeleteCompilation => "utility.autodelete_compilation",
        SettingKey::UtilityGitText => "utility.git_text",
        SettingKey::UtilityAutomaticNaming => "utility.automatic_naming",
        SettingKey::GitAutoCommit => "git.auto_commit",
        SettingKey::GitAutoBranch => "git.auto_branch",
        SettingKey::GitAutoPush => "git.auto_push",
        SettingKey::GitAutoDraftPullRequest => "git.auto_draft_pull_request",
        SettingKey::GitBranchPrefix => "git.branch_prefix",
        SettingKey::GitMessageInstructions => "git.message_instructions",
        SettingKey::SecurityAuditRetentionDays => "security.audit_retention_days",
        SettingKey::RetentionTrashGraceDays => "retention.trash_grace_days",
        SettingKey::DeveloperMode => "craft.developer_mode",
        SettingKey::AutomaticReview => "review.automatic",
        SettingKey::AutomaticReviewBinding => "review.account_binding",
        SettingKey::AutomaticReviewConsent => "review.cross_provider_consent",
    }
}

fn parse_key(value: &str) -> Result<SettingKey, PublicError> {
    if value.is_empty() || value.len() > MAX_KEY_BYTES {
        return Err(key_invalid());
    }
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(|_| key_invalid())
}

/// Keys whose value is empty or one Account binding UUID
/// (`jet-core/src/setting/key.rs`, `setting.binding_invalid`).
fn names_binding(key: SettingKey) -> bool {
    matches!(
        key,
        SettingKey::UtilityAccountBinding
            | SettingKey::UtilityContentConsent
            | SettingKey::AutomaticReviewBinding
            | SettingKey::AutomaticReviewConsent
    )
}

/// Text the shell will show and write back unchanged: at most 2,048 bytes
/// and no control character, except line breaks and tabs in Git message
/// instructions. Empty text is valid (it is several keys' built-in).
fn text_fits(key: SettingKey, text: &str) -> bool {
    text.len() <= MAX_SETTING_TEXT_BYTES
        && text.chars().all(|character| {
            !character.is_control()
                || (key == SettingKey::GitMessageInstructions
                    && (character == '\n' || character == '\t'))
        })
}

/// A Setting's text as a view. A value that fails the display rules is
/// `Undisplayable`; no fallback string is substituted into something the
/// row could write back.
fn setting_text_view(key: SettingKey, text: &str) -> ValueView {
    if text_fits(key, text) {
        ValueView::Text(text.to_owned())
    } else {
        ValueView::Undisplayable
    }
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36 && Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

/// Shape and size only. Floors and policy come back from `jetd`.
fn validate_value(
    key: SettingKey,
    current: &SettingValue,
    value: &SettingValue,
) -> Result<(), PublicError> {
    let same_shape = matches!(
        (current, value),
        (SettingValue::Flag(_), SettingValue::Flag(_))
            | (SettingValue::Text(_), SettingValue::Text(_))
            | (SettingValue::Count(_), SettingValue::Count(_))
    );
    if !same_shape {
        return Err(value_invalid());
    }
    if let SettingValue::Text(text) = value {
        if !text_fits(key, text)
            || (names_binding(key) && !text.is_empty() && !canonical_uuid(text))
        {
            return Err(value_invalid());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// What the Plane's status says about changes. `unknown` is an older minor
/// or a store Recovery has not validated. No breach detail crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaneStateView {
    security: &'static str,
    recovery: &'static str,
}

pub(crate) fn plane_state(status: &PlaneStatus) -> PlaneStateView {
    PlaneStateView {
        security: match status.security {
            Some(SecurityState::Trusted) => "trusted",
            Some(SecurityState::Degraded { .. }) => "degraded",
            None => "unknown",
        },
        recovery: match status.recovery.as_ref().map(|recovery| recovery.state) {
            Some(RecoveryState::Serving) => "serving",
            Some(RecoveryState::ReadOnly) => "read_only",
            None => "unknown",
        },
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshotView {
    snapshot_id: String,
    cursor: String,
    scope: ScopeView,
    plane_state: PlaneStateView,
    settings: Vec<ResolvedSettingView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ScopeView {
    Plane,
    Project { project_id: String },
    Conversation { conversation_id: String },
}

impl From<&SettingScope> for ScopeView {
    fn from(scope: &SettingScope) -> Self {
        match scope {
            SettingScope::Plane => Self::Plane,
            SettingScope::Project { project_id } => Self::Project {
                project_id: project_id.to_string(),
            },
            SettingScope::Conversation { conversation_id } => Self::Conversation {
                conversation_id: conversation_id.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolvedSettingView {
    key: &'static str,
    value: ValueView,
    source: SourceView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum ValueView {
    Flag(bool),
    Text(String),
    Count(u32),
    Undisplayable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "source",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum SourceView {
    BuiltIn,
    Plane,
    Project { project_id: String },
    Conversation { conversation_id: String },
}

fn value_view(key: SettingKey, value: &SettingValue) -> ValueView {
    match value {
        SettingValue::Flag(flag) => ValueView::Flag(*flag),
        SettingValue::Text(text) => setting_text_view(key, text),
        SettingValue::Count(count) => ValueView::Count(*count),
    }
}

fn source_view(source: &SettingSource) -> SourceView {
    match source {
        SettingSource::BuiltIn => SourceView::BuiltIn,
        SettingSource::Scope { scope } => match scope {
            SettingScope::Plane => SourceView::Plane,
            SettingScope::Project { project_id } => SourceView::Project {
                project_id: project_id.to_string(),
            },
            SettingScope::Conversation { conversation_id } => SourceView::Conversation {
                conversation_id: conversation_id.to_string(),
            },
        },
    }
}

fn resolved_view(setting: &ResolvedSetting) -> ResolvedSettingView {
    ResolvedSettingView {
        key: key_spelling(setting.key),
        value: value_view(setting.key, &setting.value),
        source: source_view(&setting.source),
    }
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SettingChangePreparation {
    /// Nothing changed since the snapshot: this exact change may be applied.
    Review { review: Box<SettingsReviewView> },
    /// The Plane's value is no longer the one the row showed.
    Changed { current: ResolvedSettingView },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsReviewView {
    review_id: String,
    subject: ReviewSubject,
    /// Setting reviews only.
    before: Option<ResolvedSettingView>,
    /// Setting reviews only. `None` clears the scope's value, so the
    /// inherited value applies.
    after: Option<ValueView>,
}

impl SettingsReviewView {
    /// A review of an action that is not one Setting: its exact values are
    /// in the subject.
    pub(crate) fn action(review_id: Uuid, subject: ReviewSubject) -> Self {
        Self {
            review_id: review_id.to_string(),
            subject,
            before: None,
            after: None,
        }
    }
}

/// What a review will do, as exact labeled facts. Nothing in it is ever
/// sent back: apply takes only the review ID.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ReviewSubject {
    Setting {
        key: &'static str,
        scope: ScopeView,
    },
    Bind {
        provider: &'static str,
        harness: &'static str,
    },
    AutoContinue {
        binding_id: String,
        before: AutoContinuePolicyView,
        after: AutoContinuePolicyView,
    },
    Unbind {
        binding_id: String,
        label: String,
        provider: String,
        credential_source: &'static str,
    },
    DisableCraft {
        craft_id: String,
        harness_names: Vec<String>,
        mode: &'static str,
    },
    InstallCraft {
        preview: CraftPreviewView,
    },
    ChangeExtension {
        preview: ExtensionReviewView,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SettingsReceipt {
    Applied {
        detail: AppliedDetail,
    },
    /// A definite refusal: nothing changed. A new prepare may follow.
    Refused {
        error: PublicError,
    },
    /// The apply-time guard found a newer value. Nothing was sent.
    Changed {
        current: ResolvedSettingView,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AppliedDetail {
    /// `value` is `None` after a clear.
    Setting {
        key: &'static str,
        value: Option<ValueView>,
    },
    AccountBound {
        binding_id: String,
    },
    AutoContinue,
    /// What is left for the user to clean up: `none`,
    /// `keyring_item_remains`, `helper` or `session`.
    AccountUnbound {
        cleanup: &'static str,
    },
    CraftDisabled,
    CraftInstallQueued {
        craft_id: String,
        version: String,
    },
    /// The Craft applies it once running tasks finish; its status is read
    /// with `load_extension_change`.
    ExtensionChangeQueued {
        change_id: String,
    },
}

// ---------------------------------------------------------------------------
// Webview inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ScopeInput {
    // Struct variants, so `deny_unknown_fields` also covers them.
    Plane {},
    Project { project_id: String },
}

/// Only Plane and Project scopes are editable in this app (wave 3.2 §11.7).
fn parse_scope(value: serde_json::Value) -> Result<SettingScope, PublicError> {
    match serde_json::from_value::<ScopeInput>(value).map_err(|_| scope_invalid())? {
        ScopeInput::Plane {} => Ok(SettingScope::Plane),
        ScopeInput::Project { project_id } if canonical_uuid(&project_id) => {
            Ok(SettingScope::Project {
                project_id: Uuid::parse_str(&project_id).map_err(|_| scope_invalid())?,
            })
        }
        ScopeInput::Project { .. } => Err(scope_invalid()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ChangeInput {
    Set { value: SettingValue },
    Clear {},
}

fn parse_change(value: serde_json::Value) -> Result<ChangeInput, PublicError> {
    serde_json::from_value(value).map_err(|_| value_invalid())
}

// ---------------------------------------------------------------------------
// Native ledger
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct SettingsState {
    snapshots: Mutex<HashMap<Uuid, SnapshotGrant>>,
    reviews: Mutex<HashMap<Uuid, SettingsReview>>,
    order: AtomicU64,
    watch: SettingsWatch,
}

struct SnapshotGrant {
    plane: PlaneKey,
    scope: SettingScope,
    values: Vec<ResolvedSetting>,
    created_at: Instant,
}

/// What a snapshot lets a prepare act on.
struct SnapshotLookup {
    plane: PlaneKey,
    scope: SettingScope,
    values: Vec<ResolvedSetting>,
}

/// The exact typed request a review sends. It is stored natively and sent
/// byte-equal on every attempt, under the review ID as its command ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsAction {
    Set {
        key: SettingKey,
        scope: SettingScope,
        value: SettingValue,
    },
    Clear {
        key: SettingKey,
        scope: SettingScope,
    },
    /// A Harness-native binding: never a credential.
    Bind {
        provider: &'static str,
        label: &'static str,
    },
    AutoContinue {
        binding_id: Uuid,
        policy: AutoContinuePolicy,
    },
    Unbind {
        binding_id: Uuid,
    },
    DisableCraft {
        craft_id: String,
        mode: CraftDisableMode,
    },
    /// The confirmation stays native; the webview only saw its facts.
    InstallCraft {
        confirmation: CraftInstallationConfirmation,
    },
    /// The exact inspection `jetd` and the Craft revalidate. Built natively
    /// from an inspection grant; the webview only saw its facts.
    ChangeExtension {
        confirmation: ExtensionConfirmation,
    },
}

impl SettingsAction {
    pub(crate) fn slot(&self) -> Slot {
        match self {
            Self::Set { key, scope, .. } | Self::Clear { key, scope } => {
                Slot::Setting(*scope, *key)
            }
            Self::Bind { provider, .. } => Slot::Bind(provider),
            Self::AutoContinue { binding_id, .. } => Slot::AutoContinue(*binding_id),
            Self::Unbind { binding_id } => Slot::Account(*binding_id),
            Self::DisableCraft { craft_id, .. } => Slot::Craft(craft_id.clone()),
            Self::InstallCraft { .. } => Slot::CraftInstall,
            Self::ChangeExtension { .. } => Slot::Extension,
        }
    }

    /// The Setting a Set or Clear changes, which the fresh-read guard reads.
    fn setting(&self) -> Option<(SettingKey, SettingScope)> {
        match self {
            Self::Set { key, scope, .. } | Self::Clear { key, scope } => Some((*key, *scope)),
            _ => None,
        }
    }
}

/// One attempted, unresolved change is allowed per slot on each Plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Slot {
    Setting(SettingScope, SettingKey),
    Bind(&'static str),
    AutoContinue(Uuid),
    Account(Uuid),
    Craft(String),
    CraftInstall,
    /// Every extension change of a Plane: one uncertain change at a time.
    Extension,
}

/// The value the user saw when the review was admitted. Checked again on
/// the first apply only: after an uncertain send the value may already be
/// the user's own.
struct FreshGuard {
    seen: ResolvedSetting,
}

struct SettingsReview {
    order: u64,
    plane: PlaneKey,
    action: SettingsAction,
    guard: Option<FreshGuard>,
    created_at: Instant,
    attempted: bool,
    result: Option<SettingsReceipt>,
    /// When the receipt was recorded. A resolved review is kept only long
    /// enough to answer a lost IPC reply.
    resolved_at: Option<Instant>,
}

impl SettingsReview {
    fn unresolved(&self) -> bool {
        self.attempted && self.result.is_none()
    }

    /// Unresolved reviews are the only record of an uncertain send and are
    /// never dropped. Others live for `REVIEW_LIFETIME`: from admission
    /// until they are resolved, then from their receipt.
    fn retained(&self, now: Instant) -> bool {
        self.unresolved()
            || now.saturating_duration_since(self.resolved_at.unwrap_or(self.created_at))
                < REVIEW_LIFETIME
    }

    fn occupies(&self, plane: PlaneId, slot: &Slot) -> bool {
        self.plane.plane == plane && self.action.slot() == *slot && self.unresolved()
    }
}

/// What an apply does next.
enum Begin {
    /// A receipt from an earlier attempt: the IPC reply was lost.
    Known(SettingsReceipt),
    Send {
        plane: PlaneKey,
        action: SettingsAction,
        /// Present on the first attempt only.
        guard: Option<ResolvedSetting>,
    },
}

impl SettingsState {
    fn next_order(&self) -> u64 {
        self.order.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn grant_snapshot(
        &self,
        plane: PlaneKey,
        scope: SettingScope,
        values: Vec<ResolvedSetting>,
        now: Instant,
    ) -> Result<Uuid, PublicError> {
        let mut snapshots = self.snapshots.lock().map_err(|_| PublicError::internal())?;
        snapshots
            .retain(|_, grant| now.saturating_duration_since(grant.created_at) < SNAPSHOT_LIFETIME);
        while snapshots.len() >= SNAPSHOT_CAPACITY {
            let Some(oldest) = snapshots
                .iter()
                .min_by_key(|(_, grant)| grant.created_at)
                .map(|(id, _)| *id)
            else {
                break;
            };
            snapshots.remove(&oldest);
        }
        let id = Uuid::new_v4();
        snapshots.insert(
            id,
            SnapshotGrant {
                plane,
                scope,
                values,
                created_at: now,
            },
        );
        Ok(id)
    }

    /// A snapshot issued for `plane`. A snapshot of another Plane is as
    /// unknown as an expired one.
    fn snapshot(
        &self,
        id: Uuid,
        plane: PlaneId,
        now: Instant,
    ) -> Result<SnapshotLookup, PublicError> {
        let snapshots = self.snapshots.lock().map_err(|_| PublicError::internal())?;
        let grant = snapshots
            .get(&id)
            .filter(|grant| grant.plane.plane == plane)
            .filter(|grant| now.saturating_duration_since(grant.created_at) < SNAPSHOT_LIFETIME)
            .ok_or_else(snapshot_expired)?;
        Ok(SnapshotLookup {
            plane: grant.plane,
            scope: grant.scope,
            values: grant.values.clone(),
        })
    }

    pub(crate) fn ensure_slot_free(&self, plane: PlaneId, slot: Slot) -> Result<(), PublicError> {
        let reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        if reviews.values().any(|review| review.occupies(plane, &slot)) {
            return Err(request_unresolved());
        }
        Ok(())
    }

    /// Admits one immutable review. Overlapping unattempted reviews of the
    /// same slot stay valid; only an attempted, unresolved one blocks.
    pub(crate) fn admit(
        &self,
        plane: PlaneKey,
        action: SettingsAction,
        seen: Option<ResolvedSetting>,
        now: Instant,
    ) -> Result<Uuid, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        let slot = action.slot();
        if reviews
            .values()
            .any(|review| review.occupies(plane.plane, &slot))
        {
            return Err(request_unresolved());
        }
        reviews.retain(|_, review| review.retained(now));
        if reviews.len() >= REVIEW_CAPACITY {
            // Never evict an unresolved review. Unattempted reviews go
            // first, then the oldest resolved receipts.
            let oldest = reviews
                .iter()
                .filter(|(_, review)| !review.attempted)
                .min_by_key(|(_, review)| review.order)
                .or_else(|| {
                    reviews
                        .iter()
                        .filter(|(_, review)| review.result.is_some())
                        .min_by_key(|(_, review)| review.order)
                })
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    reviews.remove(&id);
                }
                None => return Err(request_limit()),
            }
        }
        let id = Uuid::new_v4();
        reviews.insert(
            id,
            SettingsReview {
                order: self.next_order(),
                plane,
                action,
                guard: seen.map(|seen| FreshGuard { seen }),
                created_at: now,
                attempted: false,
                result: None,
                resolved_at: None,
            },
        );
        Ok(id)
    }

    fn begin(&self, id: Uuid, plane: PlaneId, now: Instant) -> Result<Begin, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        let review = reviews
            .get(&id)
            .filter(|review| review.plane.plane == plane)
            .ok_or_else(review_expired)?;
        if let Some(known) = &review.result {
            return Ok(Begin::Known(known.clone()));
        }
        if !review.attempted && now.saturating_duration_since(review.created_at) >= REVIEW_LIFETIME
        {
            reviews.remove(&id);
            return Err(review_expired());
        }
        let slot = review.action.slot();
        if reviews
            .iter()
            .any(|(other, candidate)| *other != id && candidate.occupies(plane, &slot))
        {
            return Err(request_unresolved());
        }
        let review = reviews.get(&id).ok_or_else(PublicError::internal)?;
        Ok(Begin::Send {
            plane: review.plane,
            action: review.action.clone(),
            guard: if review.attempted {
                None
            } else {
                review.guard.as_ref().map(|guard| guard.seen.clone())
            },
        })
    }

    fn mark_attempted(&self, id: Uuid) -> Result<(), PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        let review = reviews.get_mut(&id).ok_or_else(review_expired)?;
        review.attempted = true;
        Ok(())
    }

    fn record(&self, id: Uuid, receipt: &SettingsReceipt, now: Instant) {
        if let Ok(mut reviews) = self.reviews.lock() {
            if let Some(review) = reviews.get_mut(&id) {
                review.result = Some(receipt.clone());
                review.resolved_at = Some(now);
            }
        }
    }

    fn discard(&self, id: Uuid) {
        if let Ok(mut reviews) = self.reviews.lock() {
            reviews.remove(&id);
        }
    }
}

/// Values compared by the fresh-read guard: the value and where it came
/// from. A value set again at another scope is a different view.
fn same_view(left: &ResolvedSetting, right: &ResolvedSetting) -> bool {
    left.key == right.key && left.value == right.value && left.source == right.source
}

/// Reads one key at one scope on an open connection. Also gates the key's
/// minor through `jet-client`.
async fn read_one(
    client: &Connection,
    key: SettingKey,
    scope: SettingScope,
) -> Result<ResolvedSetting, Box<ClientError>> {
    let snapshot = client
        .query(client.settings(scope, SettingSelection::Key { key }))
        .await?;
    let mut matching = snapshot
        .settings
        .into_iter()
        .filter(|setting| setting.key == key);
    match (matching.next(), matching.next()) {
        (Some(setting), None) if snapshot.scope == scope => Ok(setting),
        _ => Err(Box::new(ClientError::Unexpected(
            "settings key snapshot".into(),
        ))),
    }
}

/// A definite daemon refusal ends a review; anything else stays uncertain.
fn definite(error: &ClientError, public: &PublicError) -> bool {
    matches!(error, ClientError::Remote(_)) && public.category != "outcome_unknown"
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn load_settings(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    scope: serde_json::Value,
) -> Result<SettingsSnapshotView, PublicError> {
    load_settings_for(&bridge, &plane_id, scope).await
}

async fn load_settings_for(
    bridge: &JetBridge,
    plane_id: &str,
    scope: serde_json::Value,
) -> Result<SettingsSnapshotView, PublicError> {
    let scope = parse_scope(scope)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    async {
        let client = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let status = client
            .query(client.status())
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        bridge.planes.observe_status(binding.plane, &status);
        let snapshot = client
            .query(client.settings(scope, SettingSelection::All))
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        if snapshot.scope != scope || snapshot.settings.len() > MAX_SNAPSHOT_SETTINGS {
            return Err(PublicError::internal());
        }
        let mut values: Vec<ResolvedSetting> = Vec::with_capacity(snapshot.settings.len());
        for setting in snapshot.settings {
            // One value per key; a repeated key is not a snapshot.
            if values.iter().any(|seen| seen.key == setting.key) {
                return Err(PublicError::internal());
            }
            values.push(setting);
        }
        let settings = values.iter().map(resolved_view).collect();
        let snapshot_id = bridge
            .settings
            .grant_snapshot(binding, scope, values, Instant::now())?;
        Ok(SettingsSnapshotView {
            snapshot_id: snapshot_id.to_string(),
            cursor: snapshot.cursor.to_string(),
            scope: ScopeView::from(&scope),
            plane_state: plane_state(&status),
            settings,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn prepare_setting_change(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    snapshot_id: String,
    key: String,
    change: serde_json::Value,
) -> Result<SettingChangePreparation, PublicError> {
    prepare_setting_change_for(&bridge, &plane_id, &snapshot_id, &key, change).await
}

async fn prepare_setting_change_for(
    bridge: &JetBridge,
    plane_id: &str,
    snapshot_id: &str,
    key: &str,
    change: serde_json::Value,
) -> Result<SettingChangePreparation, PublicError> {
    let plane = plane_handle(plane_id)?;
    let snapshot_id = Uuid::parse_str(snapshot_id).map_err(|_| snapshot_expired())?;
    let key = parse_key(key)?;
    let change = parse_change(change)?;
    let snapshot = bridge
        .settings
        .snapshot(snapshot_id, plane, Instant::now())?;
    if !allowed(key, &snapshot.scope) {
        return Err(scope_invalid());
    }
    let before = snapshot
        .values
        .iter()
        .find(|setting| setting.key == key)
        .cloned()
        .ok_or_else(key_unavailable)?;
    let scope = snapshot.scope;
    let action = match change {
        ChangeInput::Set { value } => {
            validate_value(key, &before.value, &value)?;
            SettingsAction::Set { key, scope, value }
        }
        ChangeInput::Clear {} => SettingsAction::Clear { key, scope },
    };
    bridge.settings.ensure_slot_free(plane, action.slot())?;

    let binding = snapshot.plane;
    async {
        let client = bridge.bound(&binding)?;
        let connection = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let fresh = read_one(&connection, key, scope)
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        // Fresh-read guard (prepare): never admit a change to a value the
        // user did not see.
        if !same_view(&fresh, &before) {
            return Ok(SettingChangePreparation::Changed {
                current: resolved_view(&fresh),
            });
        }
        let after = match &action {
            SettingsAction::Set { value, .. } => Some(value_view(key, value)),
            _ => None,
        };
        let review_id =
            bridge
                .settings
                .admit(binding, action, Some(fresh.clone()), Instant::now())?;
        Ok(SettingChangePreparation::Review {
            review: Box::new(SettingsReviewView {
                review_id: review_id.to_string(),
                subject: ReviewSubject::Setting {
                    key: key_spelling(key),
                    scope: ScopeView::from(&scope),
                },
                before: Some(resolved_view(&fresh)),
                after,
            }),
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn apply_settings_change(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    review_id: String,
) -> Result<SettingsReceipt, PublicError> {
    apply_settings_change_for(&bridge, &plane_id, &review_id).await
}

pub(crate) async fn apply_settings_change_for(
    bridge: &JetBridge,
    plane_id: &str,
    review_id: &str,
) -> Result<SettingsReceipt, PublicError> {
    let plane = plane_handle(plane_id)?;
    let Ok(id) = Uuid::parse_str(review_id) else {
        return Ok(SettingsReceipt::Refused {
            error: review_expired(),
        });
    };
    let (binding, action, guard) = match bridge.settings.begin(id, plane, Instant::now()) {
        Ok(Begin::Known(receipt)) => return Ok(receipt),
        Ok(Begin::Send {
            plane,
            action,
            guard,
        }) => (plane, action, guard),
        // Nothing was sent: the review is gone, expired or blocked.
        Err(error) => return Ok(SettingsReceipt::Refused { error }),
    };
    // The reviewed change goes only to the Plane it was reviewed against.
    let client = match bridge.bound(&binding) {
        Ok(client) => client,
        Err(error) => {
            let receipt = SettingsReceipt::Refused { error };
            bridge.settings.record(id, &receipt, Instant::now());
            return Ok(receipt);
        }
    };
    send_reviewed(bridge, id, binding, client, action, guard)
        .await
        .map_err(|error| bridge.settle(&binding, error))
}

async fn send_reviewed(
    bridge: &JetBridge,
    id: Uuid,
    binding: PlaneKey,
    client: PlaneClient,
    action: SettingsAction,
    guard: Option<ResolvedSetting>,
) -> Result<SettingsReceipt, PublicError> {
    let connection = client
        .connect()
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    if let (Some(seen), Some((key, scope))) = (guard, action.setting()) {
        // Fresh-read guard (apply), first attempt only: closes the window
        // between the review and the confirm.
        match read_one(&connection, key, scope).await {
            Ok(fresh) if same_view(&fresh, &seen) => {}
            Ok(fresh) => {
                bridge.settings.discard(id);
                return Ok(SettingsReceipt::Changed {
                    current: resolved_view(&fresh),
                });
            }
            Err(error) => {
                let public = bridge.settle(&binding, PublicError::from_client(&error));
                if definite(&error, &public) {
                    let receipt = SettingsReceipt::Refused { error: public };
                    bridge.settings.record(id, &receipt, Instant::now());
                    return Ok(receipt);
                }
                return Err(public);
            }
        }
    }
    bridge.settings.mark_attempted(id)?;
    let receipt = match send_action(&connection, id, &action).await {
        Ok(Some(detail)) => {
            if let (
                SettingsAction::ChangeExtension { confirmation },
                AppliedDetail::ExtensionChangeQueued { change_id },
            ) = (&action, &detail)
            {
                if let Ok(change_id) = Uuid::parse_str(change_id) {
                    bridge
                        .extensions
                        .record_change(binding, change_id, confirmation);
                }
            }
            SettingsReceipt::Applied { detail }
        }
        // The daemon answered, but not with what was asked: a definite
        // outcome the shell refuses to trust.
        Ok(None) => SettingsReceipt::Refused {
            error: PublicError::internal(),
        },
        Err(error) => {
            let public = bridge.settle(&binding, PublicError::from_client(&error));
            if !definite(&error, &public) {
                // Uncertain: the same review resends the same body.
                return Err(public);
            }
            SettingsReceipt::Refused { error: public }
        }
    };
    bridge.settings.record(id, &receipt, Instant::now());
    Ok(receipt)
}

/// Sends one reviewed action. ADR-0093: the review UUID is the command ID,
/// and the body is the exact stored request on every attempt. `Ok(None)`
/// is an answer that does not match the request.
async fn send_action(
    connection: &Connection,
    id: Uuid,
    action: &SettingsAction,
) -> Result<Option<AppliedDetail>, Box<ClientError>> {
    Ok(match action {
        SettingsAction::Set { key, scope, value } => {
            let stored = connection
                .command(connection.set_setting(id, *key, *scope, value.clone()))
                .await?;
            (std::mem::discriminant(value) == std::mem::discriminant(&stored)).then(|| {
                AppliedDetail::Setting {
                    key: key_spelling(*key),
                    value: Some(value_view(*key, &stored)),
                }
            })
        }
        SettingsAction::Clear { key, scope } => {
            connection
                .command(connection.clear_setting(id, *key, *scope))
                .await?;
            Some(AppliedDetail::Setting {
                key: key_spelling(*key),
                value: None,
            })
        }
        SettingsAction::Bind { provider, label } => {
            // ASVS 13.3.1: Harness-native only; no credential crosses.
            let bound = connection
                .command(connection.bind_account(
                    id,
                    provider,
                    label,
                    None,
                    CredentialSource::HarnessNative,
                ))
                .await?;
            (bound.provider == *provider).then(|| AppliedDetail::AccountBound {
                binding_id: bound.binding_id.to_string(),
            })
        }
        SettingsAction::AutoContinue { binding_id, policy } => {
            connection
                .command(connection.set_auto_continue(
                    id,
                    AutoContinueTarget::AccountBinding(*binding_id),
                    policy.clone(),
                ))
                .await?;
            Some(AppliedDetail::AutoContinue)
        }
        SettingsAction::Unbind { binding_id } => {
            let reference = connection
                .command(connection.unbind_account(id, *binding_id))
                .await?;
            Some(AppliedDetail::AccountUnbound {
                cleanup: agents::cleanup_after_unbind(&reference),
            })
        }
        SettingsAction::DisableCraft { craft_id, mode } => {
            connection
                .command(connection.disable_craft(id, craft_id.clone(), *mode))
                .await?;
            Some(AppliedDetail::CraftDisabled)
        }
        SettingsAction::InstallCraft { confirmation } => {
            let queued = connection
                .command(connection.install_craft(id, confirmation.clone()))
                .await?;
            // The queued Artifact must be the one the user reviewed.
            (queued.artifact_sha256 == confirmation.artifact_sha256).then(|| {
                AppliedDetail::CraftInstallQueued {
                    craft_id: agents::bounded_label(&queued.craft_id, 128, "Craft"),
                    version: agents::bounded_label(&queued.version, 64, "Unknown"),
                }
            })
        }
        SettingsAction::ChangeExtension { confirmation } => {
            let change_id = connection
                .command(connection.change_extension(id, confirmation.clone()))
                .await?;
            Some(AppliedDetail::ExtensionChangeQueued {
                change_id: change_id.to_string(),
            })
        }
    })
}

// ---------------------------------------------------------------------------
// Work context
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkContextView {
    plane_state: PlaneStateView,
    projects: Vec<ProjectOptionView>,
    projects_cursor: Option<String>,
    bindings: Vec<BindingOptionView>,
    autodelete: Option<AutodeleteSummaryView>,
    issues: Vec<WorkIssueView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectOptionView {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingOptionView {
    id: String,
    label: String,
    provider: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutodeleteSummaryView {
    count: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkIssueView {
    section: &'static str,
    error: PublicError,
}

/// Bounded display text; an empty, oversized or control-bearing value is
/// replaced by `fallback`. Used for labels only, never for Setting values.
fn safe_label(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if !value.is_empty()
        && value.len() <= maximum_bytes
        && value.chars().all(|character| !character.is_control())
    {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

#[tauri::command]
pub(crate) async fn load_work_context(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<WorkContextView, PublicError> {
    load_work_context_for(&bridge, &plane_id).await
}

async fn load_work_context_for(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<WorkContextView, PublicError> {
    let (binding, client) = plane_client(bridge, plane_id)?;
    let settle = |error: PublicError| bridge.settle(&binding, error);
    let client = client
        .connect()
        .await
        .map_err(|e| settle(PublicError::from_client(&e)))?;
    let status = client
        .query(client.status())
        .await
        .map_err(|e| settle(PublicError::from_client(&e)))?;
    bridge.planes.observe_status(binding.plane, &status);
    let mut issues = Vec::new();

    let (projects, projects_cursor) = match client.query(client.projects()).await {
        Ok(list) => (
            list.projects
                .iter()
                .take(MAX_PROJECTS)
                .map(|project| ProjectOptionView {
                    id: project.project_id.to_string(),
                    // The root path never crosses; only its final component.
                    name: project_name(&project.root),
                })
                .collect(),
            Some(list.cursor.to_string()),
        ),
        Err(error) => {
            issues.push(WorkIssueView {
                section: "projects",
                error: settle(PublicError::from_client(&error)),
            });
            (Vec::new(), None)
        }
    };

    let bindings = match client
        .query(client.account_bindings(CapabilityObservation::LastObserved))
        .await
    {
        Ok(list) => list
            .bindings
            .iter()
            .take(MAX_BINDINGS)
            .map(|status| BindingOptionView {
                id: status.binding.binding_id.to_string(),
                label: safe_label(&status.binding.label, 96, "Account"),
                provider: safe_label(&status.binding.provider, 64, "Provider"),
            })
            .collect(),
        Err(error) => {
            issues.push(WorkIssueView {
                section: "accounts",
                error: settle(PublicError::from_client(&error)),
            });
            Vec::new()
        }
    };

    let autodelete = match client.query(client.autodelete_rules()).await {
        Ok(rules) => Some(AutodeleteSummaryView {
            count: u32::try_from(rules.rules.len()).unwrap_or(u32::MAX),
        }),
        Err(error) => {
            issues.push(WorkIssueView {
                section: "autodelete",
                error: settle(PublicError::from_client(&error)),
            });
            None
        }
    };

    Ok(WorkContextView {
        plane_state: plane_state(&status),
        projects,
        projects_cursor,
        bindings,
        autodelete,
        issues,
    })
}

// ---------------------------------------------------------------------------
// Change watcher (§4.5)
// ---------------------------------------------------------------------------

/// Event kinds that can make a Settings section stale. There is no policy,
/// Craft or extension kind; those sections reload on focus instead.
const STALENESS_KINDS: [&str; 13] = [
    "setting.changed",
    "setting.cleared",
    "account.bound",
    "account.unbound",
    "auto_continue.changed",
    "auto_continue.configured",
    "project.registered",
    "project.removed",
    "usage.recorded",
    "audit.epoch_begun",
    "schedule.created",
    "schedule.canceled",
    "schedule.fired",
];

fn staleness_kind(kind: &str) -> Option<&'static str> {
    STALENESS_KINDS.iter().copied().find(|known| *known == kind)
}

/// What the Settings window learns from the Plane's journal: kinds and
/// Setting identifiers only. No timeline, text, Conversation or Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SettingsChange {
    Resumed {
        after: String,
    },
    Change {
        sequence: String,
        kind: &'static str,
        setting_key: Option<&'static str>,
        setting_scope: Option<&'static str>,
        project_id: Option<String>,
    },
    Reconnecting {
        error: PublicError,
    },
    Failed {
        error: PublicError,
    },
}

/// Projects one native update for the Settings window, or drops it.
fn settings_change(update: NativeUpdate, plane: PlaneId) -> Option<SettingsChange> {
    match update {
        NativeUpdate::Resumed { after } => Some(SettingsChange::Resumed {
            after: after.to_string(),
        }),
        NativeUpdate::Event(event) => {
            let kind = staleness_kind(&event.kind)?;
            let setting = event.setting;
            Some(SettingsChange::Change {
                sequence: event.sequence.to_string(),
                kind,
                setting_key: setting.map(|setting| setting.key),
                setting_scope: setting.map(|setting| setting.scope),
                project_id: setting
                    .and_then(|setting| setting.project_id)
                    .map(|id| id.to_string()),
            })
        }
        NativeUpdate::Reconnecting { error } => Some(SettingsChange::Reconnecting {
            error: error.with_plane(plane.to_string()),
        }),
        NativeUpdate::Failed { error } => Some(SettingsChange::Failed {
            error: error.with_plane(plane.to_string()),
        }),
    }
}

/// The one Settings watcher. It is separate from the main window's feeds:
/// it never fences or raises notifications and never replaces a feed.
#[derive(Default)]
struct SettingsWatch {
    generation: AtomicU64,
    task: Mutex<Option<(u64, AbortHandle)>>,
}

impl SettingsWatch {
    /// Starts a new generation and stops the previous watcher.
    fn begin(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut task) = self.task.lock() {
            if let Some((_, previous)) = task.take() {
                previous.abort();
            }
        }
        generation
    }

    /// Keeps `handle` unless a newer watcher started meanwhile.
    fn attach(&self, generation: u64, handle: AbortHandle) {
        match self.task.lock() {
            Ok(mut task) if self.generation.load(Ordering::SeqCst) == generation => {
                if let Some((_, previous)) = task.replace((generation, handle)) {
                    previous.abort();
                }
            }
            _ => handle.abort(),
        }
    }

    fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut task) = self.task.lock() {
            if let Some((_, previous)) = task.take() {
                previous.abort();
            }
        }
    }
}

impl SettingsState {
    /// The Settings window was destroyed: its watcher has no receiver.
    pub(crate) fn window_destroyed(&self) {
        self.watch.stop();
    }
}

#[tauri::command]
pub(crate) async fn watch_settings_changes(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    after: String,
    on_change: Channel<SettingsChange>,
) -> Result<(), PublicError> {
    watch_settings_changes_for(&bridge, &plane_id, after, move |change| {
        on_change.send(change).is_ok()
    })
}

fn watch_settings_changes_for(
    bridge: &JetBridge,
    plane_id: &str,
    after: String,
    mut send: impl FnMut(SettingsChange) -> bool + Send + 'static,
) -> Result<(), PublicError> {
    let cursor = parse_resume_cursor(Some(after))?.unwrap_or_default();
    let (binding, client) = plane_client(bridge, plane_id)?;
    let plane = binding.plane;
    let watch = &bridge.settings.watch;
    let generation = watch.begin();
    let task = tokio::spawn(async move {
        client
            .stream_updates(cursor, |update| match settings_change(update, plane) {
                Some(change) => send(change),
                None => true,
            })
            .await;
    });
    watch.attach(generation, task.abort_handle());
    Ok(())
}

// ---------------------------------------------------------------------------
// Stable shell codes (§4.7)
// ---------------------------------------------------------------------------

fn scope_invalid() -> PublicError {
    PublicError::invalid_input(
        "settings.scope_invalid",
        "That setting can't be changed for this Plane or Project.",
    )
}

fn snapshot_expired() -> PublicError {
    PublicError::invalid_input(
        "settings.snapshot_expired",
        "These settings are out of date. Load them again.",
    )
}

fn key_invalid() -> PublicError {
    PublicError::invalid_input("settings.key_invalid", "Jet doesn't know that setting.")
}

fn key_unavailable() -> PublicError {
    PublicError::invalid_input(
        "settings.key_unavailable",
        "This setting needs a newer Jet service on this Plane.",
    )
}

fn value_invalid() -> PublicError {
    PublicError::invalid_input(
        "settings.value_invalid",
        "That value doesn't fit this setting.",
    )
}

fn review_expired() -> PublicError {
    PublicError::invalid_input(
        "settings.review_expired",
        "This change is out of date. Make it again.",
    )
}

fn request_unresolved() -> PublicError {
    PublicError::invalid_input(
        "settings.request_unresolved",
        "Finish or retry the previous change first.",
    )
}

fn request_limit() -> PublicError {
    PublicError::invalid_input(
        "settings.request_limit",
        "Too many changes are waiting for an answer. Retry or finish them first.",
    )
}

#[cfg(test)]
impl SettingsState {
    /// The actions of every admitted review, for tests in other modules.
    pub(crate) fn reviews_for_test(&self) -> Vec<SettingsAction> {
        self.reviews
            .lock()
            .unwrap()
            .values()
            .map(|review| review.action.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests;
