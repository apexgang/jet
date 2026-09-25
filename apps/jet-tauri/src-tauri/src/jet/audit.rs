//! The owner-only Security audit (Wave 3.3 §4.5) for the Settings window's
//! Safety › Audit section: a paged, redacted viewer and a native evidence
//! export.
//!
//! Redaction happens here. Unless the user asks to see identifiers, a page
//! carries no client ID, target identity or target reference, and it never
//! carries the Plane ID. Revealed identifiers are allowlisted shapes only.
//!
//! The export is chosen in a native save dialog and written by this shell.
//! No path crosses to or from the webview; only the chosen file's name comes
//! back. The file is written beside its target as an owner-only partial file
//! created with `O_EXCL`, so an existing file or symlink there is never
//! followed, and it replaces the target only once it is complete.
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use jet_protocol::{
    AuditActor, AuditEntry, AuditOutcome, AuditRisk, SecurityAudit, SecurityState,
    SECURITY_AUDIT_MINOR,
};
use serde::Serialize;
use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::{
    client::Connection,
    errors::{safe_code, PublicError},
    planes::{PlaneBinding, PlaneId},
    settings::plane_client,
    setup::safe_text,
    system::InFlight,
    JetBridge,
};

/// Records per page, as the Plane serves them (`jet-store` audit).
const PAGE_LIMIT: usize = 256;
/// Pages an export reads at most: 1,048,576 records.
const MAX_EXPORT_PAGES: usize = 4096;
/// Longest Plane-label slug in a file name and in the export header.
const MAX_SLUG: usize = 40;
/// Hex characters of a revealed target reference that are shown.
const REFERENCE_SHOWN: usize = 16;
/// Longest target reference accepted as lowercase hex (a SHA-256).
const MAX_REFERENCE: usize = 64;
/// Longest chosen file name that is shown back.
const MAX_FILE_NAME: usize = 128;

/// Native export state per Plane. The mark gates a new audit epoch in the
/// Recovery ledger: evidence of the degraded epoch must be saved first.
#[derive(Default)]
pub(crate) struct AuditState {
    export_in_flight: Mutex<HashSet<PlaneId>>,
    exported: Mutex<HashMap<PlaneId, ExportMark>>,
}

/// What the last completed export of a Plane covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExportMark {
    /// The Plane identity the export read, so a mark never outlives it.
    pub(crate) identity: Uuid,
    /// The degraded epoch it was saved under; `None` when the audit was
    /// trusted, which unlocks nothing.
    pub(crate) epoch: Option<u64>,
    /// The audit position the export read through.
    pub(crate) through: u64,
}

impl AuditState {
    /// The last export of this Plane identity, if any.
    pub(crate) fn mark(
        &self,
        plane: PlaneId,
        identity: Uuid,
    ) -> Result<Option<ExportMark>, PublicError> {
        Ok(self
            .exported
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&plane)
            .copied()
            .filter(|mark| mark.identity == identity))
    }

    fn record(&self, plane: PlaneId, mark: ExportMark) -> Result<(), PublicError> {
        self.exported
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(plane, mark);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn record_for_test(
        &self,
        plane: PlaneId,
        mark: ExportMark,
    ) -> Result<(), PublicError> {
        self.record(plane, mark)
    }

    /// Forgets a Plane's export: its audit began a new epoch, or its store
    /// was replaced by an older snapshot.
    pub(crate) fn clear(&self, plane: PlaneId) {
        if let Ok(mut exported) = self.exported.lock() {
            exported.remove(&plane);
        }
    }
}

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuditPageView {
    /// The newest audit position when the page was read.
    cursor: String,
    /// No newer record exists beyond this page.
    complete: bool,
    /// Oldest first.
    entries: Vec<AuditEntryView>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditEntryView {
    sequence: String,
    epoch: String,
    recorded_at_unix_ms: String,
    actor: ActorView,
    target: TargetView,
    decision: String,
    risk: &'static str,
    outcome: &'static str,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActorView {
    kind: &'static str,
    client_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetView {
    kind: String,
    identity: Option<String>,
    reference: Option<String>,
}

fn page_invalid() -> PublicError {
    PublicError::invalid_response_code(
        "audit.page_invalid",
        "The Plane returned an invalid security audit page.",
    )
}

fn after_invalid() -> PublicError {
    PublicError::invalid_input(
        "audit.cursor_invalid",
        "Jet could not read the security audit from that position.",
    )
}

/// Reads one page of a Plane's Security audit strictly after `after`
/// (`None` starts from the oldest retained record). Identifiers are
/// withheld unless `reveal` is set.
#[tauri::command]
pub(crate) async fn load_security_audit(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    after: Option<String>,
    reveal: bool,
) -> Result<AuditPageView, PublicError> {
    load_page(&bridge, &plane_id, after, reveal).await
}

pub(super) async fn load_page(
    bridge: &JetBridge,
    plane_id: &str,
    after: Option<String>,
    reveal: bool,
) -> Result<AuditPageView, PublicError> {
    let after = parse_after(after)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    let this_device = client.client_id();
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let page = read_page(&connection, after).await?;
        bridge
            .planes
            .observe_success(binding.plane, SECURITY_AUDIT_MINOR);
        Ok(page_view(&page, this_device, reveal))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

/// A decimal audit position of at most 20 digits; `None` is 0.
fn parse_after(after: Option<String>) -> Result<u64, PublicError> {
    let Some(value) = after else {
        return Ok(0);
    };
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(after_invalid());
    }
    value.parse().map_err(|_| after_invalid())
}

/// One page, checked: at most [`PAGE_LIMIT`] records, strictly after
/// `after`, strictly ascending and never past the page's cursor.
async fn read_page(connection: &Connection, after: u64) -> Result<SecurityAudit, PublicError> {
    let page = connection
        .query(connection.security_audit_after(after))
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    check_page(after, &page)?;
    Ok(page)
}

fn check_page(after: u64, page: &SecurityAudit) -> Result<(), PublicError> {
    if page.entries.len() > PAGE_LIMIT {
        return Err(page_invalid());
    }
    let mut previous = after;
    for entry in &page.entries {
        if entry.sequence <= previous || entry.sequence > page.cursor {
            return Err(page_invalid());
        }
        previous = entry.sequence;
    }
    Ok(())
}

/// The last page is the one whose final record is the audit's cursor; an
/// empty page has nothing newer.
fn complete(page: &SecurityAudit) -> bool {
    page.entries
        .last()
        .is_none_or(|entry| entry.sequence == page.cursor)
}

fn page_view(page: &SecurityAudit, this_device: Uuid, reveal: bool) -> AuditPageView {
    AuditPageView {
        cursor: page.cursor.to_string(),
        complete: complete(page),
        entries: page
            .entries
            .iter()
            .map(|entry| entry_view(entry, this_device, reveal))
            .collect(),
    }
}

// ASVS 8.3.1: identifiers leave the shell only on request, and only in an
// allowlisted shape; the Plane ID never does.
fn entry_view(entry: &AuditEntry, this_device: Uuid, reveal: bool) -> AuditEntryView {
    let actor = match &entry.actor {
        AuditActor::InteractiveClient { client_id } => ActorView {
            kind: if *client_id == this_device {
                "this_device"
            } else {
                "other_client"
            },
            client_id: reveal.then(|| client_id.to_string()),
        },
        AuditActor::CraftRevocation => ActorView {
            kind: "craft_revocation",
            client_id: None,
        },
        AuditActor::Retention => ActorView {
            kind: "retention",
            client_id: None,
        },
    };
    AuditEntryView {
        sequence: entry.sequence.to_string(),
        epoch: entry.epoch.to_string(),
        recorded_at_unix_ms: entry.recorded_at_unix_ms.to_string(),
        actor,
        target: TargetView {
            kind: safe_code(&entry.target.kind).unwrap_or_else(|| "unknown".into()),
            identity: entry
                .target
                .identity
                .as_deref()
                .filter(|_| reveal)
                .and_then(|identity| Uuid::parse_str(identity).ok())
                .map(|identity| identity.to_string()),
            reference: reveal
                .then(|| shown_reference(&entry.target.reference))
                .flatten(),
        },
        decision: safe_code(&entry.decision).unwrap_or_else(|| "unknown".into()),
        risk: match entry.risk {
            AuditRisk::Routine => "routine",
            AuditRisk::Elevated => "elevated",
            AuditRisk::Destructive => "destructive",
        },
        outcome: match entry.outcome {
            AuditOutcome::Succeeded => "succeeded",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Failed => "failed",
        },
    }
}

/// A lowercase hex reference, shortened for display.
fn shown_reference(reference: &str) -> Option<String> {
    let hex = !reference.is_empty()
        && reference.len() <= MAX_REFERENCE
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !hex {
        return None;
    }
    Some(if reference.len() > REFERENCE_SHOWN {
        format!("{}…", &reference[..REFERENCE_SHOWN])
    } else {
        reference.to_owned()
    })
}

// ---------------------------------------------------------------------------
// Evidence export
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AuditExportView {
    /// The evidence file was written; only its name is returned.
    Saved { records: String, file_name: String },
    /// The user closed the save dialog.
    Canceled,
}

fn export_failed() -> PublicError {
    PublicError::local_internal(
        "audit.export_failed",
        "Jet couldn't save the audit evidence file.",
    )
}

fn export_too_large() -> PublicError {
    PublicError::invalid_input(
        "audit.export_too_large",
        "The security audit is too large to save from this app.",
    )
}

/// Saves a Plane's whole Security audit as JSON Lines to a file the user
/// picks in a native dialog. One export per Plane at a time.
#[tauri::command]
pub(crate) async fn export_security_audit(
    window: WebviewWindow,
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<AuditExportView, PublicError> {
    let (binding, client) = plane_client(&bridge, &plane_id)?;
    let _flight = enter_export(&bridge, binding.plane)?;
    // Fail before the dialog when the Plane cannot be read at all.
    let connection = client
        .connect()
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    connection
        .query(connection.status())
        .await
        .map_err(|error| bridge.settle(&binding, PublicError::from_client(&error)))?;
    drop(connection);
    let slug = slug(&bridge.planes.label(binding.plane).unwrap_or_default());
    let now = unix_ms_now();
    let Some(target) = choose_target(&window, &file_name(&slug, now)).await? else {
        return Ok(AuditExportView::Canceled);
    };
    export_to(&bridge, binding, &target, &slug, now).await
}

fn enter_export(bridge: &JetBridge, plane: PlaneId) -> Result<InFlight<'_>, PublicError> {
    InFlight::enter(
        &bridge.audit.export_in_flight,
        plane,
        PublicError::conflict(
            "audit.export_busy",
            "Jet is already saving this Plane's audit evidence.",
        ),
    )
    .map_err(|error| error.with_plane(plane.to_string()))
}

/// The native save dialog, parented to the calling window. `None` when the
/// user cancels.
async fn choose_target(
    window: &WebviewWindow,
    file_name: &str,
) -> Result<Option<PathBuf>, PublicError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(window)
        .set_title("Save audit evidence")
        .add_filter("Jet audit evidence", &["jsonl"])
        .set_file_name(file_name)
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    match receiver.await.map_err(|_| PublicError::internal())? {
        None => Ok(None),
        Some(path) => path.into_path().map(Some).map_err(|_| export_failed()),
    }
}

/// Reads the Plane's status and every audit page into `target`, then marks
/// the export so a new audit epoch may begin.
pub(super) async fn export_to(
    bridge: &JetBridge,
    binding: PlaneBinding,
    target: &Path,
    slug: &str,
    now: i64,
) -> Result<AuditExportView, PublicError> {
    async {
        let client = bridge.bound(&binding)?;
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let status = connection
            .query(connection.status())
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge.planes.observe_status(binding.plane, &status);
        // The export describes the Plane the request was resolved against.
        bridge.bound(&PlaneBinding {
            plane: binding.plane,
            identity: Some(status.plane_id),
        })?;
        let header = Header {
            format: "jet-audit-evidence",
            version: 1,
            plane_label: slug,
            security: status.security.as_ref(),
            exported_at_unix_ms: now,
        };
        let (records, through) = write_export(&connection, target, &header).await?;
        bridge
            .planes
            .observe_success(binding.plane, SECURITY_AUDIT_MINOR);
        let epoch = match status.security {
            Some(SecurityState::Degraded { epoch, .. }) => Some(epoch),
            Some(SecurityState::Trusted) | None => None,
        };
        bridge.audit.record(
            binding.plane,
            ExportMark {
                identity: status.plane_id,
                epoch,
                through,
            },
        )?;
        Ok(AuditExportView::Saved {
            records: records.to_string(),
            file_name: target
                .file_name()
                .map(|name| safe_text(&name.to_string_lossy(), MAX_FILE_NAME, "the chosen file"))
                .unwrap_or_else(|| "the chosen file".into()),
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

/// The first line of an evidence file.
#[derive(Debug, Serialize)]
struct Header<'a> {
    format: &'static str,
    version: u32,
    plane_label: &'a str,
    security: Option<&'a SecurityState>,
    exported_at_unix_ms: i64,
}

/// Streams every page after the header, one wire `AuditEntry` per line.
/// Returns the record count and the audit position read through.
async fn write_export(
    connection: &Connection,
    target: &Path,
    header: &Header<'_>,
) -> Result<(u64, u64), PublicError> {
    let mut file = EvidenceFile::create(target).await?;
    file.line(header).await?;
    let mut after = 0;
    let mut records = 0_u64;
    for _ in 0..MAX_EXPORT_PAGES {
        let page = read_page(connection, after).await?;
        for entry in &page.entries {
            file.line(entry).await?;
        }
        records += page.entries.len() as u64;
        if complete(&page) {
            file.finish().await?;
            return Ok((records, page.cursor.max(after)));
        }
        // `check_page` proved the page ascending and non-empty here.
        after = page.entries.last().map_or(after, |entry| entry.sequence);
    }
    Err(export_too_large())
}

/// An owner-only partial file beside the target. It is removed on every
/// exit path unless [`EvidenceFile::finish`] renamed it into place.
struct EvidenceFile {
    partial: PathBuf,
    target: PathBuf,
    file: Option<tokio::io::BufWriter<tokio::fs::File>>,
}

impl EvidenceFile {
    async fn create(target: &Path) -> Result<Self, PublicError> {
        let (Some(directory), Some(name)) = (target.parent(), target.file_name()) else {
            return Err(export_failed());
        };
        let mut partial_name = OsString::from(".");
        partial_name.push(name);
        partial_name.push(".partial");
        let partial = directory.join(partial_name);
        let mut options = tokio::fs::OpenOptions::new();
        // ASVS 12.3.1: O_EXCL never follows or truncates an existing file
        // or symlink at the partial path; the evidence is owner-only.
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(&partial).await.map_err(|_| export_failed())?;
        Ok(Self {
            partial,
            target: target.to_owned(),
            file: Some(tokio::io::BufWriter::new(file)),
        })
    }

    async fn line(&mut self, value: &impl Serialize) -> Result<(), PublicError> {
        let mut line = serde_json::to_vec(value).map_err(|_| PublicError::internal())?;
        line.push(b'\n');
        self.file
            .as_mut()
            .ok_or_else(PublicError::internal)?
            .write_all(&line)
            .await
            .map_err(|_| export_failed())
    }

    async fn finish(mut self) -> Result<(), PublicError> {
        let mut file = self.file.take().ok_or_else(PublicError::internal)?;
        file.flush().await.map_err(|_| export_failed())?;
        file.get_ref()
            .sync_all()
            .await
            .map_err(|_| export_failed())?;
        drop(file);
        tokio::fs::rename(&self.partial, &self.target)
            .await
            .map_err(|_| export_failed())?;
        // Renamed: nothing is left to remove.
        self.partial = PathBuf::new();
        Ok(())
    }
}

impl Drop for EvidenceFile {
    fn drop(&mut self) {
        if !self.partial.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

/// A Plane label reduced to `[a-z0-9-]`, for file names and the header.
pub(super) fn slug(label: &str) -> String {
    let mut slug = String::new();
    for character in label.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(MAX_SLUG).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "plane".into()
    } else {
        slug.to_owned()
    }
}

/// `jet-audit-<slug>-<UTC date>.jsonl`.
fn file_name(slug: &str, now_unix_ms: i64) -> String {
    let (year, month, day) = utc_date(now_unix_ms);
    format!("jet-audit-{slug}-{year:04}-{month:02}-{day:02}.jsonl")
}

fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The proleptic Gregorian UTC date of a Unix-millisecond instant
/// (Howard Hinnant's `civil_from_days`).
fn utc_date(unix_ms: i64) -> (i64, u32, u32) {
    let days = unix_ms.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests;
