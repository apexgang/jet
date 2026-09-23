//! Agents settings (wave 3.2 §4.4, slice 3): Harnesses and Crafts, Account
//! bindings, usage and Auto-continue for one Plane.
//!
//! Every mutation is a review in the settings ledger (`settings.rs`): the
//! webview sees exact facts and an opaque review ID, and `jetd` decides every
//! policy. Only Harness-native Account bindings are offered: this app never
//! handles a credential or chooses a program for `jetd` to run.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jet_protocol::{
    AccountBindingList, AccountBindingStatus, AutoContinuePolicy, AutoContinueSnapshot,
    AutoContinueStatus, AutoContinueTarget, BrokerPermission, CapabilityObservation,
    CapabilitySnapshot, CraftDisableMode, CraftHostAccess, CraftInstallationPreview, CraftSource,
    CraftTrust, CredentialReference, CredentialStoreStatus, PlaneStatus, PlaneUsage, QuotaScope,
    QuotaUnit, QuotaWindow, UsageEstimation, UsageFinality, UsageFreshness, UsageHistory,
    UsageHistoryRange, UsageHistorySelection, UsageResolution, UsageSelection, UsageTokens,
};
use serde::{Deserialize, Serialize};
use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use super::{
    errors::PublicError,
    planes::{self, PlaneId},
    settings::{
        plane_client, plane_state, PlaneKey, PlaneStateView, ReviewSubject, SettingsAction,
        SettingsReviewView,
    },
    setup::{auth_provider_options, credential_state, degraded_label},
    JetBridge,
};

/// A canonical Plane handle is `local` or a 36-byte UUID.
const MAX_PLANE_ID_BYTES: usize = 36;
const MAX_PROVIDER_BYTES: usize = 64;
const MAX_CRAFT_ID_BYTES: usize = 128;
const MAX_CRAFTS: usize = 64;
const MAX_HARNESSES: usize = 32;
const MAX_ACCOUNTS: usize = 64;
const MAX_DEGRADED: usize = 16;
const MAX_QUOTA_WINDOWS: usize = 64;
const MAX_HISTORY_SERIES: usize = 32;
const MAX_HISTORY_POINTS: usize = 2_400;
const MAX_FEATURES: usize = 32;
const MAX_HOST_ACCESS: usize = 64;
/// `docs/auto-continue.md`: delays 1–86,400,000 ms, retries 1–100, and
/// 1–8,192 bytes of nonblank message text.
pub(crate) const MIN_DELAY_MS: u32 = 1;
pub(crate) const MAX_DELAY_MS: u32 = 86_400_000;
pub(crate) const MIN_RETRIES: u32 = 1;
pub(crate) const MAX_RETRIES: u32 = 100;
pub(crate) const MAX_MESSAGE_BYTES: usize = 8_192;
const DAY_MS: i64 = 86_400_000;
/// Local Craft sources (`craft/installation.rs`, `Local`).
const MAX_SOURCE_PATH_BYTES: usize = 4_096;
const MAX_SOURCE_NAME_BYTES: usize = 128;
const SPECIFICATION_NAME: &str = "craft-spec.toml";
const LOCAL_SOURCE_CAPACITY: usize = 4;
const LOCAL_SOURCE_LIFETIME: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------------------
// Harness names
// ---------------------------------------------------------------------------

/// The first-party Harnesses this app can name and bind. The protocol has
/// no product name, so the Harness identifier is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnownHarness {
    Codex,
    ClaudeCode,
}

impl KnownHarness {
    pub(crate) fn of(harness_id: &str) -> Option<Self> {
        let normalized = harness_id.to_ascii_lowercase();
        if normalized.contains("codex") {
            Some(Self::Codex)
        } else if normalized.contains("claude") {
            Some(Self::ClaudeCode)
        } else {
            None
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
        }
    }

    /// The Account provider a Harness-native sign-in binds.
    pub(crate) fn provider(self) -> &'static str {
        match self {
            Self::Codex => "openai",
            Self::ClaudeCode => "anthropic",
        }
    }

    /// The fixed, non-secret binding label.
    pub(crate) fn login_label(self) -> &'static str {
        match self {
            Self::Codex => "Codex login",
            Self::ClaudeCode => "Claude Code login",
        }
    }
}

/// The product name of a Harness, or `None` for one this app doesn't know.
pub(crate) fn harness_display_name(harness_id: &str) -> Option<&'static str> {
    KnownHarness::of(harness_id).map(KnownHarness::name)
}

fn harness_view(harness_id: &str) -> HarnessView {
    HarnessView {
        id: bounded_label(harness_id, 64, "unknown"),
        name: harness_display_name(harness_id)
            .map(str::to_owned)
            .unwrap_or_else(|| bounded_label(harness_id, 64, "Unknown Harness")),
    }
}

// ---------------------------------------------------------------------------
// Bounded text
// ---------------------------------------------------------------------------

/// Display-only text: an empty, oversized or control-bearing value becomes
/// `fallback`. Never used for a value that could be written back.
pub(crate) fn bounded_label(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if !value.is_empty()
        && value.len() <= maximum_bytes
        && value.chars().all(|character| !character.is_control())
    {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

/// Text that must be shown exactly (an Auto-continue message) or not at all.
fn exact_text(value: &str, maximum_bytes: usize) -> Option<String> {
    (value.len() <= maximum_bytes
        && value
            .chars()
            .all(|character| !character.is_control() || character == '\n' || character == '\t'))
    .then(|| value.to_owned())
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentsView {
    plane_state: PlaneStateView,
    crafts: Vec<CraftView>,
    harnesses: Vec<HarnessView>,
    credential_store: Option<CredentialStoreView>,
    degraded: Vec<String>,
    accounts: Vec<AccountView>,
    bind_options: Vec<BindOptionView>,
    usage: Option<UsageSummaryView>,
    /// The Account binding list's cursor, for the change watcher.
    cursor: Option<String>,
    issues: Vec<AgentsIssueView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CraftView {
    craft_id: String,
    version: String,
    harnesses: Vec<HarnessView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessView {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialStoreView {
    state: &'static str,
    label: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountView {
    id: String,
    label: String,
    provider: String,
    provider_account: Option<String>,
    credential_source: &'static str,
    state: &'static str,
    state_label: &'static str,
    created_at_unix_ms: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BindOptionView {
    provider: &'static str,
    harness: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummaryView {
    cursor: String,
    tokens: TokensView,
    measurements: String,
    estimated: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokensView {
    input: String,
    cached_input: String,
    output: String,
    reasoning: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentsIssueView {
    section: &'static str,
    error: PublicError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountDetailView {
    binding_id: String,
    quota_windows: Vec<QuotaWindowView>,
    auto_continue: Option<AutoContinueView>,
    /// The Auto-continue snapshot's cursor, for the change watcher.
    cursor: Option<String>,
    issues: Vec<AgentsIssueView>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaWindowView {
    provider: String,
    window: String,
    scope: QuotaScopeView,
    unit: &'static str,
    used: String,
    limit: Option<String>,
    window_seconds: Option<String>,
    resets_at_unix_ms: Option<String>,
    estimation: &'static str,
    finality: &'static str,
    observed_at_unix_ms: String,
    /// `unreachable` never carries the Provider's reason.
    freshness: &'static str,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum QuotaScopeView {
    Account,
    Model { model: String },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoContinueView {
    policy: AutoContinuePolicyView,
    retry: Option<AutoContinueRetryView>,
}

/// An Auto-continue policy as facts. A message that can't be shown exactly
/// is `None`; the user must write a new one to change the policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AutoContinuePolicyView {
    Off,
    Retry {
        delay_ms: u32,
        max_delay_ms: u32,
        max_retries: u32,
        message: Option<String>,
    },
}

impl From<&AutoContinuePolicy> for AutoContinuePolicyView {
    fn from(policy: &AutoContinuePolicy) -> Self {
        match policy {
            AutoContinuePolicy::Off => Self::Off,
            AutoContinuePolicy::Retry {
                delay_ms,
                max_delay_ms,
                max_retries,
                message,
            } => Self::Retry {
                delay_ms: *delay_ms,
                max_delay_ms: *max_delay_ms,
                max_retries: *max_retries,
                message: exact_text(message, MAX_MESSAGE_BYTES),
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoContinueRetryView {
    status: &'static str,
    retry_count: u32,
    due_at_unix_ms: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageHistoryView {
    cursor: String,
    /// The answer's own resolution, which may be coarser than asked.
    resolution: &'static str,
    truncated: bool,
    series: Vec<UsageSeriesView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSeriesView {
    model: Option<String>,
    points: Vec<UsagePointView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsagePointView {
    start_unix_ms: String,
    tokens: TokensView,
    measurements: String,
    estimated: String,
}

/// The facts of a Craft installation preview. The confirmation itself stays
/// native; installing sends exactly what was discovered.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CraftPreviewView {
    craft_id: String,
    version: String,
    source: &'static str,
    enabled_features: Vec<String>,
    repository: String,
    publisher_claim: String,
    commit: String,
    artifact_sha256: String,
    broker_permissions: Vec<&'static str>,
    host_access: Vec<HostAccessView>,
    trust: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostAccessView {
    kind: &'static str,
    value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalCraftSourceView {
    source_token: String,
    specification_name: String,
    artifact_name: String,
}

// ---------------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------------

fn credential_store_view(status: &CredentialStoreStatus) -> CredentialStoreView {
    let (state, label) = match status {
        CredentialStoreStatus::Available { .. } => ("available", "Secure storage ready"),
        CredentialStoreStatus::Locked { .. } => ("locked", "Secure storage locked"),
        CredentialStoreStatus::Unavailable { .. } => ("unavailable", "Secure storage unavailable"),
    };
    CredentialStoreView { state, label }
}

fn credential_source_name(reference: &CredentialReference) -> &'static str {
    match reference {
        CredentialReference::PlatformStore { .. } => "platform_store",
        CredentialReference::ExternalHelper { .. } => "external_helper",
        CredentialReference::HarnessNative => "harness_native",
        CredentialReference::SessionOnly { .. } => "session_only",
    }
}

/// What the user may still need to clean up after an unbind. The keyring
/// item and helper name never cross.
pub(crate) fn cleanup_after_unbind(reference: &CredentialReference) -> &'static str {
    match reference {
        CredentialReference::PlatformStore { .. } => "keyring_item_remains",
        CredentialReference::ExternalHelper { .. } => "helper",
        CredentialReference::HarnessNative => "none",
        CredentialReference::SessionOnly { .. } => "session",
    }
}

fn account_view(status: &AccountBindingStatus) -> AccountView {
    let (state, state_label) = credential_state(&status.credential_state);
    let binding = &status.binding;
    AccountView {
        id: binding.binding_id.to_string(),
        label: bounded_label(&binding.label, 96, "Harness account"),
        provider: bounded_label(&binding.provider, 64, "Unknown provider"),
        provider_account: binding
            .provider_account
            .as_deref()
            .map(|account| bounded_label(account, 128, "Account")),
        credential_source: credential_source_name(&binding.credential_reference),
        state,
        state_label,
        created_at_unix_ms: binding.created_at_unix_ms.to_string(),
    }
}

fn tokens_view(tokens: &UsageTokens) -> TokensView {
    TokensView {
        input: tokens.input.to_string(),
        cached_input: tokens.cached_input.to_string(),
        output: tokens.output.to_string(),
        reasoning: tokens.reasoning.to_string(),
    }
}

fn agents_view(
    status: &PlaneStatus,
    capabilities: Option<CapabilitySnapshot>,
    accounts: Option<AccountBindingList>,
    usage: Option<Box<PlaneUsage>>,
    issues: Vec<AgentsIssueView>,
) -> AgentsView {
    let (crafts, harnesses, credential_store, degraded, bind_options) = match &capabilities {
        Some(capabilities) => (
            capabilities
                .crafts
                .iter()
                .take(MAX_CRAFTS)
                .map(|craft| CraftView {
                    craft_id: bounded_label(&craft.craft_id, MAX_CRAFT_ID_BYTES, "unavailable"),
                    version: bounded_label(&craft.version, 64, "Unknown"),
                    harnesses: craft
                        .harnesses
                        .iter()
                        .take(MAX_HARNESSES)
                        .map(|id| harness_view(id))
                        .collect(),
                })
                .collect(),
            capabilities
                .harnesses
                .iter()
                .take(MAX_HARNESSES)
                .map(|id| harness_view(id))
                .collect(),
            Some(credential_store_view(&capabilities.credential_store)),
            capabilities
                .degraded
                .iter()
                .take(MAX_DEGRADED)
                .map(degraded_label)
                .collect(),
            auth_provider_options(&capabilities.harnesses)
                .into_iter()
                .map(|option| BindOptionView {
                    provider: option.provider,
                    harness: option.harness,
                })
                .collect(),
        ),
        None => (Vec::new(), Vec::new(), None, Vec::new(), Vec::new()),
    };
    AgentsView {
        plane_state: plane_state(status),
        crafts,
        harnesses,
        credential_store,
        degraded,
        accounts: accounts
            .as_ref()
            .map(|list| {
                list.bindings
                    .iter()
                    .take(MAX_ACCOUNTS)
                    .map(account_view)
                    .collect()
            })
            .unwrap_or_default(),
        bind_options,
        usage: usage.map(|usage| UsageSummaryView {
            cursor: usage.cursor.to_string(),
            tokens: tokens_view(&usage.consumption.tokens),
            measurements: usage.consumption.measurements.to_string(),
            estimated: usage.consumption.estimated.to_string(),
        }),
        cursor: accounts.map(|list| list.cursor.to_string()),
        issues,
    }
}

fn quota_window_view(window: &QuotaWindow) -> QuotaWindowView {
    QuotaWindowView {
        provider: bounded_label(&window.provider, 64, "Provider"),
        window: bounded_label(&window.window, 64, "Usage window"),
        scope: match &window.scope {
            QuotaScope::ProviderAccount => QuotaScopeView::Account,
            QuotaScope::Model { model } => QuotaScopeView::Model {
                model: bounded_label(model, 96, "Model"),
            },
        },
        unit: match window.measure.unit {
            QuotaUnit::Tokens => "tokens",
            QuotaUnit::Requests => "requests",
            QuotaUnit::Credits => "credits",
            QuotaUnit::Share => "share",
        },
        used: window.measure.used.to_string(),
        limit: window.measure.limit.map(|limit| limit.to_string()),
        window_seconds: window.window_seconds.map(|seconds| seconds.to_string()),
        resets_at_unix_ms: window.resets_at_unix_ms.map(|at| at.to_string()),
        estimation: match window.estimation {
            UsageEstimation::Measured => "measured",
            UsageEstimation::Estimated => "estimated",
        },
        finality: match window.finality {
            UsageFinality::Interim => "interim",
            UsageFinality::Final => "final",
        },
        observed_at_unix_ms: window.observed_at_unix_ms.to_string(),
        // ASVS 16.2.5: the Provider's unreachable reason is native text and
        // never crosses.
        freshness: match window.freshness {
            UsageFreshness::Fresh => "fresh",
            UsageFreshness::Stale => "stale",
            UsageFreshness::Unreachable { .. } => "unreachable",
        },
    }
}

/// An Account's quota windows. A window of another binding means the answer
/// does not match the request.
fn quota_windows(
    usage: &PlaneUsage,
    binding_id: Uuid,
) -> Result<Vec<QuotaWindowView>, PublicError> {
    if usage
        .quota_windows
        .iter()
        .any(|window| window.binding_id != binding_id)
    {
        return Err(PublicError::internal());
    }
    Ok(usage
        .quota_windows
        .iter()
        .take(MAX_QUOTA_WINDOWS)
        .map(quota_window_view)
        .collect())
}

fn auto_continue_view(snapshot: &AutoContinueSnapshot) -> AutoContinueView {
    AutoContinueView {
        policy: AutoContinuePolicyView::from(&snapshot.policy),
        retry: snapshot.retry.as_ref().map(|retry| AutoContinueRetryView {
            status: match retry.status {
                AutoContinueStatus::Disabled => "disabled",
                AutoContinueStatus::Deferred => "deferred",
                AutoContinueStatus::Pending => "pending",
                AutoContinueStatus::Dispatched => "dispatched",
                AutoContinueStatus::Canceled => "canceled",
                AutoContinueStatus::Exhausted => "exhausted",
            },
            retry_count: retry.retry_count,
            due_at_unix_ms: retry.due_at_unix_ms.to_string(),
        }),
    }
}

fn resolution_name(resolution: UsageResolution) -> &'static str {
    match resolution {
        UsageResolution::Hour => "hour",
        UsageResolution::Day => "day",
    }
}

/// The history as answered, capped at 32 series and 2,400 points. When
/// capped, the most recent points are kept.
fn history_view(history: &UsageHistory) -> UsageHistoryView {
    let mut truncated = history.series.len() > MAX_HISTORY_SERIES;
    let mut budget = MAX_HISTORY_POINTS;
    let mut series = Vec::new();
    for entry in history.series.iter().take(MAX_HISTORY_SERIES) {
        let mut points: Vec<_> = entry.points.iter().collect();
        points.sort_by_key(|point| point.start_unix_ms);
        if points.len() > budget {
            truncated = true;
            points.drain(..points.len() - budget);
        }
        budget -= points.len();
        series.push(UsageSeriesView {
            model: entry
                .model
                .as_deref()
                .map(|model| bounded_label(model, 96, "Model")),
            points: points
                .into_iter()
                .map(|point| UsagePointView {
                    start_unix_ms: point.start_unix_ms.to_string(),
                    tokens: tokens_view(&point.tokens),
                    measurements: point.measurements.to_string(),
                    estimated: point.estimated.to_string(),
                })
                .collect(),
        });
    }
    UsageHistoryView {
        cursor: history.cursor.to_string(),
        resolution: resolution_name(history.resolution),
        truncated,
        series,
    }
}

fn broker_permission_name(permission: BrokerPermission) -> &'static str {
    match permission {
        BrokerPermission::ArtifactRead => "artifact_read",
        BrokerPermission::ArtifactWrite => "artifact_write",
        BrokerPermission::RemoteTools => "remote_tools",
    }
}

fn host_access_view(access: &CraftHostAccess) -> HostAccessView {
    let (kind, value) = match access {
        CraftHostAccess::Executable { name } => ("executable", name),
        CraftHostAccess::Filesystem { path } => ("filesystem", path),
        CraftHostAccess::Environment { name } => ("environment", name),
        CraftHostAccess::Network { destination } => ("network", destination),
    };
    HostAccessView {
        kind,
        value: bounded_label(value, 512, "Can't be displayed"),
    }
}

fn preview_view(preview: &CraftInstallationPreview) -> CraftPreviewView {
    let confirmation = &preview.confirmation;
    CraftPreviewView {
        craft_id: bounded_label(&preview.craft_id, MAX_CRAFT_ID_BYTES, "Can't be displayed"),
        version: bounded_label(&preview.version, 64, "Can't be displayed"),
        source: match confirmation.source {
            CraftSource::GitHubRelease { .. } => "github_release",
            CraftSource::Local { .. } => "local",
        },
        enabled_features: preview
            .enabled_features
            .iter()
            .take(MAX_FEATURES)
            .map(|feature| bounded_label(feature, 96, "Can't be displayed"))
            .collect(),
        repository: bounded_label(&confirmation.repository, 256, "Can't be displayed"),
        publisher_claim: bounded_label(&confirmation.publisher_claim, 256, "Can't be displayed"),
        commit: bounded_label(&confirmation.commit, 128, "Can't be displayed"),
        artifact_sha256: bounded_label(&confirmation.artifact_sha256, 128, "Can't be displayed"),
        broker_permissions: confirmation
            .broker_permissions
            .iter()
            .map(|permission| broker_permission_name(*permission))
            .collect(),
        host_access: confirmation
            .host_access
            .iter()
            .take(MAX_HOST_ACCESS)
            .map(host_access_view)
            .collect(),
        trust: match confirmation.trust {
            CraftTrust::SameUserExecutable => "same_user_executable",
            CraftTrust::DeveloperSource => "developer_source",
        },
    }
}

fn disable_mode_name(mode: CraftDisableMode) -> &'static str {
    match mode {
        CraftDisableMode::Wait => "wait",
        CraftDisableMode::Force => "force",
    }
}

// ---------------------------------------------------------------------------
// Webview inputs
// ---------------------------------------------------------------------------

fn parse_plane(plane_id: &str) -> Result<PlaneId, PublicError> {
    if plane_id.len() > MAX_PLANE_ID_BYTES {
        return Err(planes::unknown_plane());
    }
    PlaneId::parse(plane_id)
}

fn parse_binding(value: &str) -> Result<Uuid, PublicError> {
    match Uuid::parse_str(value) {
        Ok(id) if value.len() == 36 && id.to_string() == value => Ok(id),
        _ => Err(account_missing()),
    }
}

pub(crate) fn validate_craft_id(craft_id: &str) -> Result<(), PublicError> {
    let valid = !craft_id.is_empty()
        && craft_id.len() <= MAX_CRAFT_ID_BYTES
        && craft_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(craft_missing())
    }
}

fn parse_mode(mode: &str) -> Result<CraftDisableMode, PublicError> {
    match mode {
        "wait" => Ok(CraftDisableMode::Wait),
        "force" => Ok(CraftDisableMode::Force),
        _ => Err(mode_invalid()),
    }
}

fn parse_days(days: u16) -> Result<u16, PublicError> {
    if matches!(days, 1 | 7 | 30 | 90) {
        Ok(days)
    } else {
        Err(PublicError::invalid_input(
            "agents.days_invalid",
            "Choose the last 24 hours, 7 days, 30 days or 90 days.",
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum PolicyInput {
    // A struct variant, so `deny_unknown_fields` also covers it.
    Off {},
    Retry {
        delay_ms: u32,
        max_delay_ms: u32,
        max_retries: u32,
        message: String,
    },
}

/// Shape and the documented bounds only; `jetd` refuses anything else with
/// `auto_continue.invalid_policy`.
fn parse_policy(value: serde_json::Value) -> Result<AutoContinuePolicy, PublicError> {
    match serde_json::from_value::<PolicyInput>(value).map_err(|_| policy_invalid())? {
        PolicyInput::Off {} => Ok(AutoContinuePolicy::Off),
        PolicyInput::Retry {
            delay_ms,
            max_delay_ms,
            max_retries,
            message,
        } => {
            let delays = (MIN_DELAY_MS..=MAX_DELAY_MS).contains(&delay_ms)
                && (MIN_DELAY_MS..=MAX_DELAY_MS).contains(&max_delay_ms)
                && delay_ms <= max_delay_ms;
            let retries = (MIN_RETRIES..=MAX_RETRIES).contains(&max_retries);
            let text =
                !message.trim().is_empty() && exact_text(&message, MAX_MESSAGE_BYTES).is_some();
            if delays && retries && text {
                Ok(AutoContinuePolicy::Retry {
                    delay_ms,
                    max_delay_ms,
                    max_retries,
                    message,
                })
            } else {
                Err(policy_invalid())
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CraftSourceInput {
    GithubRelease { repository: String, tag: String },
    Local { source_token: String },
}

fn repository_part(part: &str) -> bool {
    (1..=100).contains(&part.len())
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// `owner/name` of a GitHub repository, and a printable tag without spaces.
fn validate_release(repository: &str, tag: &str) -> Result<(), PublicError> {
    let repository_valid = repository
        .split_once('/')
        .is_some_and(|(owner, name)| repository_part(owner) && repository_part(name));
    let tag_valid =
        (1..=128).contains(&tag.len()) && tag.bytes().all(|byte| byte.is_ascii_graphic());
    if repository_valid && tag_valid {
        Ok(())
    } else {
        Err(source_invalid())
    }
}

// ---------------------------------------------------------------------------
// Local Craft sources
// ---------------------------------------------------------------------------

/// Local files the user picked in a native dialog. The paths never reach
/// the webview; it holds only the token, bound to the Plane that issued it.
#[derive(Default)]
pub(crate) struct AgentsState {
    local_sources: Mutex<HashMap<Uuid, LocalSourceGrant>>,
}

struct LocalSourceGrant {
    plane: PlaneKey,
    specification: String,
    artifact: String,
    created_at: Instant,
}

impl AgentsState {
    fn grant_local_source(
        &self,
        plane: PlaneKey,
        specification: String,
        artifact: String,
        now: Instant,
    ) -> Result<Uuid, PublicError> {
        let mut grants = self
            .local_sources
            .lock()
            .map_err(|_| PublicError::internal())?;
        grants.retain(|_, grant| {
            now.saturating_duration_since(grant.created_at) < LOCAL_SOURCE_LIFETIME
        });
        while grants.len() >= LOCAL_SOURCE_CAPACITY {
            let Some(oldest) = grants
                .iter()
                .min_by_key(|(_, grant)| grant.created_at)
                .map(|(id, _)| *id)
            else {
                break;
            };
            grants.remove(&oldest);
        }
        let id = Uuid::new_v4();
        grants.insert(
            id,
            LocalSourceGrant {
                plane,
                specification,
                artifact,
                created_at: now,
            },
        );
        Ok(id)
    }

    /// Consumes a token issued for `plane`. A token of another Plane is as
    /// unknown as an expired one and is left for its own Plane.
    fn take_local_source(
        &self,
        token: Uuid,
        plane: PlaneId,
        now: Instant,
    ) -> Result<(String, String), PublicError> {
        let mut grants = self
            .local_sources
            .lock()
            .map_err(|_| PublicError::internal())?;
        let usable = grants.get(&token).is_some_and(|grant| {
            grant.plane.plane == plane
                && now.saturating_duration_since(grant.created_at) < LOCAL_SOURCE_LIFETIME
        });
        if !usable {
            return Err(source_expired());
        }
        let grant = grants.remove(&token).ok_or_else(source_expired)?;
        Ok((grant.specification, grant.artifact))
    }
}

/// A picked file as the Plane will read it: canonical (symlinks resolved), a
/// regular file, and a bounded UTF-8 path without control characters.
fn canonical_file(path: &Path) -> Result<String, PublicError> {
    // ASVS 5.3.2: canonicalize before any check.
    let canonical = std::fs::canonicalize(path).map_err(|_| source_invalid())?;
    let metadata = std::fs::metadata(&canonical).map_err(|_| source_invalid())?;
    if !metadata.is_file() {
        return Err(source_invalid());
    }
    let text = canonical.to_str().ok_or_else(source_invalid)?;
    if text.len() > MAX_SOURCE_PATH_BYTES || text.chars().any(char::is_control) {
        return Err(source_invalid());
    }
    Ok(text.to_owned())
}

/// Validates both picked files. The specification must be named
/// `craft-spec.toml`.
fn local_source_paths(
    specification: &Path,
    artifact: &Path,
) -> Result<(String, String), PublicError> {
    let specification = canonical_file(specification)?;
    if Path::new(&specification)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(SPECIFICATION_NAME)
    {
        return Err(source_invalid());
    }
    let artifact = canonical_file(artifact)?;
    Ok((specification, artifact))
}

fn file_name(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.len() <= MAX_SOURCE_NAME_BYTES {
        return bounded_label(name, MAX_SOURCE_NAME_BYTES, "Selected file");
    }
    let mut end = MAX_SOURCE_NAME_BYTES - '…'.len_utf8();
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    bounded_label(
        &format!("{}…", &name[..end]),
        MAX_SOURCE_NAME_BYTES + 3,
        "Selected file",
    )
}

/// Opens one native file dialog as a child of `window` and waits for it.
async fn pick_file(
    window: &WebviewWindow,
    title: &str,
    filter: Option<(&str, &[&str])>,
) -> Result<Option<PathBuf>, PublicError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = window.dialog().file().set_parent(window).set_title(title);
    if let Some((name, extensions)) = filter {
        dialog = dialog.add_filter(name, extensions);
    }
    dialog.pick_file(move |path| {
        let _ = sender.send(path);
    });
    match receiver.await.map_err(|_| PublicError::internal())? {
        None => Ok(None),
        Some(path) => path.into_path().map(Some).map_err(|_| source_invalid()),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn issue(section: &'static str, error: PublicError) -> AgentsIssueView {
    AgentsIssueView { section, error }
}

#[tauri::command]
pub(crate) async fn load_agents(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    fresh_credentials: bool,
) -> Result<AgentsView, PublicError> {
    load_agents_for(&bridge, &plane_id, fresh_credentials).await
}

/// One connection: status, capabilities, Account bindings and Plane usage.
/// Each failure after status is a section issue; the rest stays usable.
async fn load_agents_for(
    bridge: &JetBridge,
    plane_id: &str,
    fresh_credentials: bool,
) -> Result<AgentsView, PublicError> {
    let (binding, client) = plane_client(bridge, plane_id)?;
    let settle = |error: PublicError| bridge.settle(&binding, error);
    let client = client
        .connect()
        .await
        .map_err(|e| settle(PublicError::from_client(&e)))?;
    let status = client
        .status()
        .await
        .map_err(|e| settle(PublicError::from_client(&e)))?;
    bridge.planes.observe_status(binding.plane, &status);
    let mut issues = Vec::new();
    let capabilities = match client.capabilities(CapabilityObservation::Fresh).await {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(issue(
                "capabilities",
                settle(PublicError::from_client(&error)),
            ));
            None
        }
    };
    let observation = if fresh_credentials {
        CapabilityObservation::Fresh
    } else {
        CapabilityObservation::LastObserved
    };
    let accounts = match client.account_bindings(observation).await {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(issue("accounts", settle(PublicError::from_client(&error))));
            None
        }
    };
    let usage = match client.usage(UsageSelection::Plane).await {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(issue("usage", settle(PublicError::from_client(&error))));
            None
        }
    };
    Ok(agents_view(&status, capabilities, accounts, usage, issues))
}

#[tauri::command]
pub(crate) async fn prepare_account_bind(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    provider: String,
) -> Result<SettingsReviewView, PublicError> {
    prepare_account_bind_for(&bridge, &plane_id, &provider).await
}

/// Reviews a Harness-native binding for one of the Plane's own Harnesses.
/// The label is fixed; no credential or helper is ever part of it.
pub(crate) async fn prepare_account_bind_for(
    bridge: &JetBridge,
    plane_id: &str,
    provider: &str,
) -> Result<SettingsReviewView, PublicError> {
    if provider.is_empty() || provider.len() > MAX_PROVIDER_BYTES {
        return Err(provider_unavailable());
    }
    let (binding, client) = plane_client(bridge, plane_id)?;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let capabilities = connection
            .capabilities(CapabilityObservation::LastObserved)
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let option = auth_provider_options(&capabilities.harnesses)
            .into_iter()
            .find(|option| option.provider == provider)
            .ok_or_else(provider_unavailable)?;
        let action = SettingsAction::Bind {
            provider: option.provider,
            label: option.label,
        };
        let review_id = bridge
            .settings
            .admit(binding, action, None, Instant::now())?;
        Ok(SettingsReviewView::action(
            review_id,
            ReviewSubject::Bind {
                provider: option.provider,
                harness: option.harness,
            },
        ))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn load_account_detail(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    binding_id: String,
) -> Result<AccountDetailView, PublicError> {
    load_account_detail_for(&bridge, &plane_id, &binding_id).await
}

async fn load_account_detail_for(
    bridge: &JetBridge,
    plane_id: &str,
    binding_id: &str,
) -> Result<AccountDetailView, PublicError> {
    let binding_id = parse_binding(binding_id)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    let settle = |error: PublicError| bridge.settle(&binding, error);
    let connection = client
        .connect()
        .await
        .map_err(|e| settle(PublicError::from_client(&e)))?;
    let mut issues = Vec::new();
    let quota_windows = match connection
        .usage(UsageSelection::Binding { binding_id })
        .await
    {
        Ok(usage) => quota_windows(&usage, binding_id).unwrap_or_else(|error| {
            issues.push(issue("quota", error));
            Vec::new()
        }),
        Err(error) => {
            issues.push(issue("quota", settle(PublicError::from_client(&error))));
            Vec::new()
        }
    };
    let mut cursor = None;
    let auto_continue = match connection
        .auto_continue(AutoContinueTarget::AccountBinding(binding_id))
        .await
    {
        Ok(snapshot) => {
            cursor = Some(snapshot.cursor.to_string());
            Some(auto_continue_view(&snapshot))
        }
        Err(error) => {
            issues.push(issue(
                "auto_continue",
                settle(PublicError::from_client(&error)),
            ));
            None
        }
    };
    Ok(AccountDetailView {
        binding_id: binding_id.to_string(),
        quota_windows,
        auto_continue,
        cursor,
        issues,
    })
}

#[tauri::command]
pub(crate) async fn load_usage_history(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    binding_id: Option<String>,
    days: u16,
) -> Result<UsageHistoryView, PublicError> {
    load_usage_history_for(&bridge, &plane_id, binding_id.as_deref(), days).await
}

fn now_unix_ms() -> Result<i64, PublicError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PublicError::internal())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| PublicError::internal())
}

/// The request for the last `days`: hourly up to a week, daily beyond.
fn history_request(days: u16, now_ms: i64) -> (UsageHistoryRange, UsageResolution) {
    (
        UsageHistoryRange {
            from_unix_ms: now_ms - i64::from(days) * DAY_MS,
            until_unix_ms: now_ms,
        },
        if days <= 7 {
            UsageResolution::Hour
        } else {
            UsageResolution::Day
        },
    )
}

async fn load_usage_history_for(
    bridge: &JetBridge,
    plane_id: &str,
    binding_id: Option<&str>,
    days: u16,
) -> Result<UsageHistoryView, PublicError> {
    let days = parse_days(days)?;
    let selection = match binding_id {
        Some(value) => UsageHistorySelection::Binding {
            binding_id: parse_binding(value)?,
        },
        None => UsageHistorySelection::Plane,
    };
    let (range, resolution) = history_request(days, now_unix_ms()?);
    let (binding, client) = plane_client(bridge, plane_id)?;
    async {
        let history = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?
            .usage_history(selection, range, resolution)
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        Ok(history_view(&history))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn prepare_auto_continue(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    binding_id: String,
    policy: serde_json::Value,
) -> Result<SettingsReviewView, PublicError> {
    prepare_auto_continue_for(&bridge, &plane_id, &binding_id, policy).await
}

async fn prepare_auto_continue_for(
    bridge: &JetBridge,
    plane_id: &str,
    binding_id: &str,
    policy: serde_json::Value,
) -> Result<SettingsReviewView, PublicError> {
    let binding_id = parse_binding(binding_id)?;
    let policy = parse_policy(policy)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    let action = SettingsAction::AutoContinue {
        binding_id,
        policy: policy.clone(),
    };
    bridge
        .settings
        .ensure_slot_free(binding.plane, action.slot())?;
    async {
        // The typed read gates Auto-continue's minor before any Command.
        let current = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?
            .auto_continue(AutoContinueTarget::AccountBinding(binding_id))
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let review_id = bridge
            .settings
            .admit(binding, action, None, Instant::now())?;
        Ok(SettingsReviewView::action(
            review_id,
            ReviewSubject::AutoContinue {
                binding_id: binding_id.to_string(),
                before: AutoContinuePolicyView::from(&current.policy),
                after: AutoContinuePolicyView::from(&policy),
            },
        ))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn prepare_account_unbind(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    binding_id: String,
) -> Result<SettingsReviewView, PublicError> {
    prepare_account_unbind_for(&bridge, &plane_id, &binding_id).await
}

async fn prepare_account_unbind_for(
    bridge: &JetBridge,
    plane_id: &str,
    binding_id: &str,
) -> Result<SettingsReviewView, PublicError> {
    let binding_id = parse_binding(binding_id)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    let action = SettingsAction::Unbind { binding_id };
    bridge
        .settings
        .ensure_slot_free(binding.plane, action.slot())?;
    async {
        let list = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?
            .account_bindings(CapabilityObservation::LastObserved)
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let status = list
            .bindings
            .iter()
            .find(|status| status.binding.binding_id == binding_id)
            .ok_or_else(account_missing)?;
        let account = account_view(status);
        let review_id = bridge
            .settings
            .admit(binding, action, None, Instant::now())?;
        Ok(SettingsReviewView::action(
            review_id,
            ReviewSubject::Unbind {
                binding_id: account.id,
                label: account.label,
                provider: account.provider,
                credential_source: account.credential_source,
            },
        ))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn prepare_craft_disable(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    craft_id: String,
    mode: String,
) -> Result<SettingsReviewView, PublicError> {
    prepare_craft_disable_for(&bridge, &plane_id, &craft_id, &mode).await
}

async fn prepare_craft_disable_for(
    bridge: &JetBridge,
    plane_id: &str,
    craft_id: &str,
    mode: &str,
) -> Result<SettingsReviewView, PublicError> {
    validate_craft_id(craft_id)?;
    let mode = parse_mode(mode)?;
    let (binding, client) = plane_client(bridge, plane_id)?;
    let action = SettingsAction::DisableCraft {
        craft_id: craft_id.to_owned(),
        mode,
    };
    bridge
        .settings
        .ensure_slot_free(binding.plane, action.slot())?;
    async {
        let capabilities = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?
            .capabilities(CapabilityObservation::Fresh)
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let craft = capabilities
            .crafts
            .iter()
            .find(|craft| craft.craft_id == craft_id)
            .ok_or_else(craft_missing)?;
        let mut seen = HashSet::new();
        let harness_names = craft
            .harnesses
            .iter()
            .take(MAX_HARNESSES)
            .map(|id| harness_view(id).name)
            .filter(|name| seen.insert(name.clone()))
            .collect();
        let review_id = bridge
            .settings
            .admit(binding, action, None, Instant::now())?;
        Ok(SettingsReviewView::action(
            review_id,
            ReviewSubject::DisableCraft {
                craft_id: craft_id.to_owned(),
                harness_names,
                mode: disable_mode_name(mode),
            },
        ))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn pick_local_craft_source(
    window: WebviewWindow,
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<Option<LocalCraftSourceView>, PublicError> {
    let binding = local_plane(&bridge, &plane_id)?;
    let Some(specification) = pick_file(
        &window,
        "Choose craft-spec.toml",
        Some(("Craft specification", &["toml"])),
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(artifact) = pick_file(&window, "Choose the built Craft executable", None).await?
    else {
        return Ok(None);
    };
    let (specification, artifact) =
        tokio::task::spawn_blocking(move || local_source_paths(&specification, &artifact))
            .await
            .map_err(|_| PublicError::internal())??;
    let specification_name = file_name(&specification);
    let artifact_name = file_name(&artifact);
    let token =
        bridge
            .agents
            .grant_local_source(binding, specification, artifact, Instant::now())?;
    Ok(Some(LocalCraftSourceView {
        source_token: token.to_string(),
        specification_name,
        artifact_name,
    }))
}

/// Local files are only meaningful to the Plane on this computer.
fn local_plane(bridge: &JetBridge, plane_id: &str) -> Result<PlaneKey, PublicError> {
    if parse_plane(plane_id)? != PlaneId::Local {
        return Err(PublicError::invalid_input(
            "agents.local_source_remote",
            "Local files can only be added to the Plane on this computer.",
        ));
    }
    plane_client(bridge, plane_id).map(|(binding, _)| binding)
}

#[tauri::command]
pub(crate) async fn discover_craft(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    source: serde_json::Value,
) -> Result<SettingsReviewView, PublicError> {
    discover_craft_for(&bridge, &plane_id, source).await
}

async fn discover_craft_for(
    bridge: &JetBridge,
    plane_id: &str,
    source: serde_json::Value,
) -> Result<SettingsReviewView, PublicError> {
    let input = serde_json::from_value::<CraftSourceInput>(source).map_err(|_| source_invalid())?;
    if let CraftSourceInput::GithubRelease { repository, tag } = &input {
        validate_release(repository, tag)?;
    }
    let (binding, client) = plane_client(bridge, plane_id)?;
    bridge
        .settings
        .ensure_slot_free(binding.plane, super::settings::Slot::CraftInstall)?;
    let source = match input {
        CraftSourceInput::GithubRelease { repository, tag } => {
            CraftSource::GitHubRelease { repository, tag }
        }
        CraftSourceInput::Local { source_token } => {
            let token = Uuid::parse_str(&source_token).map_err(|_| source_expired())?;
            // ASVS 5.3.2: the paths come from the native picker's grant for
            // this Plane, never from the webview.
            let (specification, artifact) =
                bridge
                    .agents
                    .take_local_source(token, binding.plane, Instant::now())?;
            CraftSource::Local {
                specification,
                artifact,
            }
        }
    };
    async {
        let preview = client
            .connect()
            .await
            .map_err(|e| PublicError::from_client(&e))?
            .discover_craft(source)
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        let view = preview_view(&preview);
        let review_id = bridge.settings.admit(
            binding,
            SettingsAction::InstallCraft {
                confirmation: preview.confirmation,
            },
            None,
            Instant::now(),
        )?;
        Ok(SettingsReviewView::action(
            review_id,
            ReviewSubject::InstallCraft { preview: view },
        ))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

// ---------------------------------------------------------------------------
// Stable shell codes (§4.7)
// ---------------------------------------------------------------------------

fn provider_unavailable() -> PublicError {
    PublicError::invalid_input(
        "account.provider_unavailable",
        "That Harness is not available on this Plane.",
    )
}

fn account_missing() -> PublicError {
    PublicError::invalid_input(
        "agents.account_missing",
        "That account is no longer connected to this Plane.",
    )
}

fn craft_missing() -> PublicError {
    PublicError::invalid_input(
        "agents.craft_missing",
        "That Craft is no longer installed on this Plane.",
    )
}

fn mode_invalid() -> PublicError {
    PublicError::invalid_input(
        "agents.mode_invalid",
        "Choose whether running tasks finish first.",
    )
}

fn policy_invalid() -> PublicError {
    PublicError::invalid_input(
        "agents.policy_invalid",
        "Use delays from 1 ms to 24 hours, 1 to 100 retries, and a message of up to 8,192 bytes.",
    )
}

fn source_invalid() -> PublicError {
    PublicError::invalid_input(
        "agents.source_invalid",
        "Choose a GitHub repository and tag, or a craft-spec.toml file and a built Craft file.",
    )
}

fn source_expired() -> PublicError {
    PublicError::invalid_input(
        "agents.source_expired",
        "The chosen files expired. Choose them again.",
    )
}

#[cfg(test)]
pub(crate) mod tests;
