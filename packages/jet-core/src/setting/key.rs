//! The Setting catalogue, defaults, and per-key validation.

use super::{
	MAX_SETTING_TEXT_BYTES, SettingScope, SettingScopeKind, SettingValue,
};
use crate::error::CoreError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt::Write as _;

/// A Setting this core understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
	/// Plane-wide disposable Artifact and cache budget in MiB.
	StorageDisposableMiB,
	/// Plane-wide admission ceiling for concurrent managed work.
	EnergyConcurrency,
	/// Reduced ceiling while the Plane is power constrained; zero pauses new work.
	EnergyLowPowerConcurrency,
	/// Explicitly constrain power regardless of the operating system observation.
	EnergyConstrained,
	/// Visible, opt-in permission for foreground user work to exceed the ceiling.
	EnergyForegroundOverride,
	/// Maximum ingested Artifact size in MiB.
	ArtifactMaxMiB,
	/// Newly ingested Artifact bytes per Run in MiB.
	ArtifactRunMiB,
	/// Enable Utility Git text independently of automatic committing.
	UtilityGitText,
	/// Persist disclosure and consent for sending Conversation content to this exact Utility binding.
	UtilityContentConsent,
	/// The single Plane-wide Utility Account binding, empty to disable routing.
	UtilityAccountBinding,
	/// Enable natural-language Autodelete compilation.
	UtilityAutodeleteCompilation,
	/// Whether the Utility model names Conversations automatically.
	UtilityAutomaticNaming,
	/// Whether Jet commits Harness changes without being asked (ADR-0029).
	GitAutoCommit,
	/// Create a branch lazily after a successful turn.
	GitAutoBranch,
	/// Push successful turn changes without forcing.
	GitAutoPush,
	/// Create or update the Conversation GitHub draft.
	GitAutoDraftPullRequest,
	/// Editable prefix for proposed Conversation branches.
	GitBranchPrefix,
	/// Plane-wide guidance for generated commit messages and pull-request
	/// text.
	GitMessageInstructions,
	/// How many days the Plane keeps its Security audit (ADR-0105).
	SecurityAuditRetentionDays,
	/// Whether local and source-built third-party Crafts may be installed.
	DeveloperMode,
	/// Whether this Plane reviews eligible approval requests automatically
	/// instead of waiting for a person (ADR-0012).
	AutomaticReview,
	/// The Account binding Automatic review uses, empty to keep each Run's
	/// own binding.
	AutomaticReviewBinding,
	/// Persistent consent to review through that binding when it is not the
	/// Run's own Provider.
	AutomaticReviewConsent,
}

/// Fewest days a Plane may keep its Security audit. Below this the audit
/// stops being able to answer what happened during an incident, so the
/// floor is not a preference (ADR-0105).
pub(super) const MINIMUM_AUDIT_RETENTION_DAYS: u32 = 90;

/// How many days a Plane keeps its Security audit unless a value says
/// otherwise.
pub(super) const DEFAULT_AUDIT_RETENTION_DAYS: u32 = 365;

/// One Setting's built-in default, spelled so the catalog stays constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BuiltIn {
	/// A yes-or-no default.
	Flag(bool),
	/// A text default.
	Text(&'static str),
	/// A whole-number default.
	Count(u32),
}

/// What one [`SettingKey`] declares: its durable spelling, the scopes that
/// may store it, and the default beneath every stored value.
pub(super) struct Catalog {
	key: SettingKey,
	spelling: &'static str,
	scopes: &'static [SettingScopeKind],
	built_in: BuiltIn,
}

/// Every Setting this core resolves, in the order a snapshot reports them.
pub(super) const CATALOG: [Catalog; 23] = [
	Catalog {
		key: SettingKey::StorageDisposableMiB,
		spelling: "storage.disposable_mib",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Count(5120),
	},
	Catalog {
		key: SettingKey::GitAutoBranch,
		spelling: "git.auto_branch",
		scopes: &[SettingScopeKind::Project, SettingScopeKind::Conversation],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		key: SettingKey::GitAutoPush,
		spelling: "git.auto_push",
		scopes: &[SettingScopeKind::Project, SettingScopeKind::Conversation],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		key: SettingKey::GitAutoDraftPullRequest,
		spelling: "git.auto_draft_pull_request",
		scopes: &[SettingScopeKind::Project, SettingScopeKind::Conversation],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		key: SettingKey::GitBranchPrefix,
		spelling: "git.branch_prefix",
		scopes: &[SettingScopeKind::Project, SettingScopeKind::Conversation],
		built_in: BuiltIn::Text("jet/"),
	},
	Catalog {
		key: SettingKey::UtilityAutomaticNaming,
		spelling: "utility.automatic_naming",
		scopes: &[
			SettingScopeKind::Plane,
			SettingScopeKind::Project,
			SettingScopeKind::Conversation,
		],
		built_in: BuiltIn::Flag(true),
	},
	Catalog {
		// ADR-0029 gives Git automation Project defaults that one
		// Conversation may override, and no Plane-wide value.
		key: SettingKey::GitAutoCommit,
		spelling: "git.auto_commit",
		scopes: &[SettingScopeKind::Project, SettingScopeKind::Conversation],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		// Git message instructions are Plane-wide by definition.
		key: SettingKey::GitMessageInstructions,
		spelling: "git.message_instructions",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Text(""),
	},
	Catalog {
		// One audit covers the whole Plane, so its window is Plane-wide.
		key: SettingKey::SecurityAuditRetentionDays,
		spelling: "security.audit_retention_days",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Count(DEFAULT_AUDIT_RETENTION_DAYS),
	},
	Catalog {
		key: SettingKey::UtilityGitText,
		spelling: "utility.git_text",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		key: SettingKey::UtilityContentConsent,
		spelling: "utility.content_consent",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Text(""),
	},
	Catalog {
		key: SettingKey::UtilityAccountBinding,
		spelling: "utility.account_binding",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Text(""),
	},
	Catalog {
		key: SettingKey::UtilityAutodeleteCompilation,
		spelling: "utility.autodelete_compilation",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		// Source provenance changes the Plane's executable trust boundary, so
		// Developer Mode is an explicit Plane-wide choice and defaults off.
		key: SettingKey::DeveloperMode,
		spelling: "craft.developer_mode",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		// ADR-0012 makes Automatic review one mode for the whole Plane, so
		// no narrower scope can turn it on for part of it. It defaults off:
		// deciding for a person is something a person turns on.
		key: SettingKey::AutomaticReview,
		spelling: "review.automatic",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		// Empty keeps each Run's own binding, which is the default reviewer.
		key: SettingKey::AutomaticReviewBinding,
		spelling: "review.account_binding",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Text(""),
	},
	Catalog {
		key: SettingKey::AutomaticReviewConsent,
		spelling: "review.cross_provider_consent",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Text(""),
	},
	Catalog {
		key: SettingKey::ArtifactMaxMiB,
		spelling: "artifact.max_mib",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Count(512),
	},
	Catalog {
		key: SettingKey::ArtifactRunMiB,
		spelling: "artifact.run_mib",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Count(2048),
	},
	Catalog {
		key: SettingKey::EnergyConcurrency,
		spelling: "energy.concurrency",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Count(8),
	},
	Catalog {
		key: SettingKey::EnergyLowPowerConcurrency,
		spelling: "energy.low_power_concurrency",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Count(1),
	},
	Catalog {
		key: SettingKey::EnergyConstrained,
		spelling: "energy.constrained",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Flag(false),
	},
	Catalog {
		key: SettingKey::EnergyForegroundOverride,
		spelling: "energy.foreground_override",
		scopes: &[SettingScopeKind::Plane],
		built_in: BuiltIn::Flag(false),
	},
];

impl BuiltIn {
	fn value(self) -> SettingValue {
		match self {
			Self::Flag(flag) => SettingValue::Flag(flag),
			Self::Text(text) => SettingValue::Text(text.into()),
			Self::Count(count) => SettingValue::Count(count),
		}
	}
}

impl SettingKey {
	/// The durable spelling, also used in the journal and on the wire.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		self.catalog().spelling
	}

	fn catalog(self) -> &'static Catalog {
		CATALOG
			.iter()
			.find(|entry| entry.key == self)
			.unwrap_or_else(|| unreachable!("every key is in the catalog"))
	}

	fn parse(spelling: &str) -> Option<Self> {
		CATALOG
			.iter()
			.find(|entry| entry.spelling == spelling)
			.map(|entry| entry.key)
	}

	/// The value used when no scope stores one.
	pub(super) fn built_in(self) -> SettingValue {
		self.catalog().built_in.value()
	}

	fn accepts(self, kind: SettingScopeKind) -> bool {
		self.catalog().scopes.contains(&kind)
	}

	/// Refuses a scope that may not store this Setting.
	///
	/// # Errors
	///
	/// Returns an `invalid_input` [`CoreError`] naming the scopes the
	/// Setting is restricted to.
	pub(super) fn require_scope(
		self,
		scope: SettingScope,
	) -> Result<(), CoreError> {
		if self.accepts(scope.kind()) {
			return Ok(());
		}
		let mut allowed = String::new();
		for (index, kind) in self.catalog().scopes.iter().enumerate() {
			let separator = if index == 0 { "" } else { ", " };
			let _ = write!(allowed, "{separator}{}", kind.as_str());
		}
		Err(CoreError::invalid_input(
			"setting.scope_unsupported",
			format!(
				"the Setting {} is stored at the {allowed} scope only, not \
				 the {} scope",
				self.as_str(),
				scope.kind().as_str()
			),
		))
	}

	/// Refuses a value the Setting cannot hold.
	///
	/// # Errors
	///
	/// Returns an `invalid_input` [`CoreError`] when the value has the wrong
	/// shape or exceeds the bound on stored text.
	pub(super) fn require_value(
		self,
		value: &SettingValue,
	) -> Result<(), CoreError> {
		if self == Self::GitBranchPrefix
			&& let SettingValue::Text(prefix) = value
		{
			crate::git_delivery::io::validate_operation(
				&crate::GitOperation::Branch {
					name: format!(
						"{prefix}00000000-0000-0000-0000-000000000000"
					),
				},
			)?;
		}
		if matches!(
			self,
			Self::UtilityAccountBinding
				| Self::UtilityContentConsent
				| Self::AutomaticReviewBinding
				| Self::AutomaticReviewConsent
		) && let SettingValue::Text(text) = value
			&& !text.is_empty()
			&& uuid::Uuid::parse_str(text).is_err()
		{
			return Err(CoreError::invalid_input(
				"setting.binding_invalid",
				"select an Account binding UUID, or empty text to clear it",
			));
		}
		let expected = self.catalog().built_in.value();
		if std::mem::discriminant(&expected) != std::mem::discriminant(value) {
			return Err(CoreError::invalid_input(
				"setting.value_unsupported",
				format!(
					"the Setting {} does not hold that kind of value",
					self.as_str()
				),
			));
		}
		match value {
			SettingValue::Flag(_) => Ok(()),
			SettingValue::Text(text)
				if text.len() <= MAX_SETTING_TEXT_BYTES =>
			{
				Ok(())
			}
			SettingValue::Text(_) => Err(CoreError::invalid_input(
				"setting.value_too_long",
				format!(
					"the Setting {} holds at most {MAX_SETTING_TEXT_BYTES} \
					 bytes of text",
					self.as_str()
				),
			)),
			SettingValue::Count(days)
				if self == Self::SecurityAuditRetentionDays
					&& *days < MINIMUM_AUDIT_RETENTION_DAYS =>
			{
				Err(CoreError::invalid_input(
					"setting.value_below_minimum",
					format!(
						"the Security audit is kept at least \
						 {MINIMUM_AUDIT_RETENTION_DAYS} days"
					),
				))
			}
			SettingValue::Count(_) => Ok(()),
		}
	}

	/// Every Setting the core understands, in catalog order.
	pub(super) fn all() -> Vec<Self> {
		CATALOG.iter().map(|entry| entry.key).collect()
	}
}

impl Serialize for SettingKey {
	fn serialize<S: Serializer>(
		&self,
		serializer: S,
	) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(self.as_str())
	}
}

impl<'de> Deserialize<'de> for SettingKey {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		let spelling = String::deserialize(deserializer)?;
		Self::parse(&spelling).ok_or_else(|| {
			de::Error::custom(format!("unknown Setting {spelling:?}"))
		})
	}
}
