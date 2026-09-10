//! Wire form of mutable Settings (ADR-0085).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a Setting value lives. A Command writes exactly the scope it
/// names; a Query resolves the Plane's values and then the values of the
/// scope it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingScope {
	/// Everything on the Plane.
	Plane,
	/// One registered Project.
	Project {
		/// The Project the values apply to.
		project_id: Uuid,
	},
	/// One Conversation.
	Conversation {
		/// The Conversation the values apply to.
		conversation_id: Uuid,
	},
}

/// A Setting this protocol minor names. A spelling it does not name is
/// refused rather than guessed (ADR-0094).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SettingKey {
	/// Maximum simultaneous managed work.
	#[serde(rename = "energy.concurrency")]
	EnergyConcurrency,
	/// Reduced admission ceiling under power constraints.
	#[serde(rename = "energy.low_power_concurrency")]
	EnergyLowPowerConcurrency,
	/// Force the reduced budget regardless of power observation.
	#[serde(rename = "energy.constrained")]
	EnergyConstrained,
	/// Explicitly allow foreground user work above the budget.
	#[serde(rename = "energy.foreground_override")]
	EnergyForegroundOverride,

	/// Plane-wide maximum ingested Artifact size in MiB.
	#[serde(rename = "artifact.max_mib")]
	ArtifactMaxMiB,
	/// Plane-wide newly ingested bytes per Run in MiB.
	#[serde(rename = "artifact.run_mib")]
	ArtifactRunMiB,
	/// Plane-wide utility.account_binding policy (Utility minor).
	#[serde(rename = "utility.account_binding")]
	UtilityAccountBinding,
	/// Plane-wide utility.content_consent policy (Utility minor).
	#[serde(rename = "utility.content_consent")]
	UtilityContentConsent,
	/// Plane-wide utility.autodelete_compilation policy (Utility minor).
	#[serde(rename = "utility.autodelete_compilation")]
	UtilityAutodeleteCompilation,
	/// Plane-wide utility.git_text policy (Utility minor).
	#[serde(rename = "utility.git_text")]
	UtilityGitText,

	/// Whether the Utility model names Conversations automatically.
	#[serde(rename = "utility.automatic_naming")]
	UtilityAutomaticNaming,
	/// Whether Jet commits Harness changes without being asked.
	#[serde(rename = "git.auto_commit")]
	GitAutoCommit,
	/// Create a branch lazily after a successful turn.
	#[serde(rename = "git.auto_branch")]
	GitAutoBranch,
	/// Push successful turn changes without forcing.
	#[serde(rename = "git.auto_push")]
	GitAutoPush,
	/// Create or update the Conversation GitHub draft.
	#[serde(rename = "git.auto_draft_pull_request")]
	GitAutoDraftPullRequest,
	/// Editable prefix for proposed Conversation branches.
	#[serde(rename = "git.branch_prefix")]
	GitBranchPrefix,
	/// Plane-wide guidance for generated commit messages and pull-request
	/// text.
	#[serde(rename = "git.message_instructions")]
	GitMessageInstructions,
	/// How many days the Plane keeps its Security audit.
	#[serde(rename = "security.audit_retention_days")]
	SecurityAuditRetentionDays,
	/// Whether local and source-built third-party Crafts may be installed.
	#[serde(rename = "craft.developer_mode")]
	DeveloperMode,
	/// Whether this Plane reviews eligible approval requests automatically.
	#[serde(rename = "review.automatic")]
	AutomaticReview,
	/// Plane-wide review.account_binding reviewer selection.
	#[serde(rename = "review.account_binding")]
	AutomaticReviewBinding,
	/// Persistent consent to review through a binding outside the Run's own
	/// Provider.
	#[serde(rename = "review.cross_provider_consent")]
	AutomaticReviewConsent,
}

/// One Setting's value. Each key holds exactly one of these shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
	/// A yes-or-no choice.
	Flag(bool),
	/// Bounded free text.
	Text(String),
	/// A whole number of something, such as days.
	Count(u32),
}

/// Which Settings one Query resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingSelection {
	/// Every Setting the addressed scope may store.
	All,
	/// One Setting, refused when the addressed scope may not store it.
	Key {
		/// The Setting to resolve.
		key: SettingKey,
	},
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SettingSource {
	/// The Plane's built-in default; no scope stores a value.
	BuiltIn,
	/// The value one scope stores.
	Scope {
		/// The scope that stores it.
		scope: SettingScope,
	},
}

/// One Setting as it applies to the scope a Query addressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ResolvedSetting {
	/// The Setting.
	pub key: SettingKey,
	/// Its value after precedence.
	pub value: SettingValue,
	/// The scope the value came from.
	pub source: SettingSource,
}

/// Settings resolved for one scope, fenced by a journal cursor (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SettingSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// The scope the Settings were resolved for.
	pub scope: SettingScope,
	/// The resolved Settings.
	pub settings: Vec<ResolvedSetting>,
}
