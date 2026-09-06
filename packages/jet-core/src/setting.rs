//! Mutable Settings: the catalog the core understands, the scopes that may
//! store each one, and how a Query resolves them (ADR-0085).
//!
//! `~/.jet/config.toml` keeps only the bootstrap values `jetd` needs before
//! its store opens. Everything here is mutable Plane state that changes
//! through authenticated Commands and resolves from built-in defaults
//! through the Plane, Project, and Conversation scopes, except where a key
//! is restricted to narrower ones.
//!
//! A restriction says where a value may be *stored*, so a Command that
//! names an unsupported scope is refused. It never narrows what applies: a
//! Plane-wide value still resolves for a Conversation that cannot override
//! it.

use std::fmt::Write as _;

use jet_store::{ReadTransaction, SettingRecord, SettingScopeRecord};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::ProjectId;
use crate::conversation::ConversationId;
use crate::error::CoreError;
use crate::event::{EventSequence, EventSubject};

/// Largest text value one Setting may carry. The store bounds the encoded
/// row as well, with room for JSON escaping above this limit.
const MAX_SETTING_TEXT_BYTES: usize = 2048;

/// Where a Setting value lives. A Command writes exactly the scope it
/// names; a Query resolves the Plane's values and then the values of the
/// scope it names (ADR-0085).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum SettingScope {
	/// Everything on the Plane.
	Plane,
	/// One registered Project.
	Project {
		/// The Project the values apply to.
		project_id: ProjectId,
	},
	/// One Conversation.
	Conversation {
		/// The Conversation the values apply to.
		conversation_id: ConversationId,
	},
}

/// A scope without its subject identity, used to declare where a key may
/// be stored and to order one scope against another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SettingScopeKind {
	/// Everything on the Plane; the widest scope above the built-in default.
	Plane,
	/// One registered Project.
	Project,
	/// One Conversation; the narrowest scope, resolved last.
	Conversation,
}

impl SettingScope {
	fn kind(self) -> SettingScopeKind {
		match self {
			Self::Plane => SettingScopeKind::Plane,
			Self::Project { .. } => SettingScopeKind::Project,
			Self::Conversation { .. } => SettingScopeKind::Conversation,
		}
	}

	pub(crate) fn record(self) -> SettingScopeRecord {
		match self {
			Self::Plane => SettingScopeRecord::Plane,
			Self::Project { project_id } => SettingScopeRecord::Project {
				project_id: project_id.0,
			},
			Self::Conversation { conversation_id } => {
				SettingScopeRecord::Conversation {
					conversation_id: conversation_id.0,
				}
			}
		}
	}

	fn from_record(record: SettingScopeRecord) -> Self {
		match record {
			SettingScopeRecord::Plane => Self::Plane,
			SettingScopeRecord::Project { project_id } => Self::Project {
				project_id: ProjectId(project_id),
			},
			SettingScopeRecord::Conversation { conversation_id } => {
				Self::Conversation {
					conversation_id: ConversationId(conversation_id),
				}
			}
		}
	}
}

impl SettingScopeKind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Plane => "Plane",
			Self::Project => "Project",
			Self::Conversation => "Conversation",
		}
	}
}

/// A Setting this core understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
	/// Whether the Utility model names Conversations automatically.
	UtilityAutomaticNaming,
	/// Whether Jet commits Harness changes without being asked (ADR-0029).
	GitAutoCommit,
	/// Plane-wide guidance for generated commit messages and pull-request
	/// text.
	GitMessageInstructions,
	/// How many days the Plane keeps its Security audit (ADR-0105).
	SecurityAuditRetentionDays,
}

/// Fewest days a Plane may keep its Security audit. Below this the audit
/// stops being able to answer what happened during an incident, so the
/// floor is not a preference (ADR-0105).
const MINIMUM_AUDIT_RETENTION_DAYS: u32 = 90;

/// How many days a Plane keeps its Security audit unless a value says
/// otherwise.
const DEFAULT_AUDIT_RETENTION_DAYS: u32 = 365;

/// One Setting's built-in default, spelled so the catalog stays constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltIn {
	/// A yes-or-no default.
	Flag(bool),
	/// A text default.
	Text(&'static str),
	/// A whole-number default.
	Count(u32),
}

/// What one [`SettingKey`] declares: its durable spelling, the scopes that
/// may store it, and the default beneath every stored value.
struct Catalog {
	key: SettingKey,
	spelling: &'static str,
	scopes: &'static [SettingScopeKind],
	built_in: BuiltIn,
}

/// Every Setting this core resolves, in the order a snapshot reports them.
const CATALOG: [Catalog; 4] = [
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
	fn built_in(self) -> SettingValue {
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
	fn require_scope(self, scope: SettingScope) -> Result<(), CoreError> {
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
	fn require_value(self, value: &SettingValue) -> Result<(), CoreError> {
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
	fn all() -> Vec<Self> {
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

/// One Setting's value. Each key holds exactly one of these shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
	/// A yes-or-no choice.
	Flag(bool),
	/// Bounded free text.
	Text(String),
	/// A whole number of something, such as days.
	Count(u32),
}

impl SettingValue {
	fn encode(&self) -> Result<String, CoreError> {
		serde_json::to_string(self).map_err(|error| {
			CoreError::internal("setting.value_unencodable", error.to_string())
		})
	}

	fn decode(text: &str) -> Option<Self> {
		serde_json::from_str(text).ok()
	}
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
	/// The core's built-in default; no scope stores a value.
	BuiltIn,
	/// The value one scope stores.
	Scope(SettingScope),
}

/// One Setting as it applies to the scope a Query addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSetting {
	/// The Setting.
	pub key: SettingKey,
	/// Its value after precedence.
	pub value: SettingValue,
	/// The scope the value came from.
	pub source: SettingSource,
}

/// Which Settings one Query resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSelection {
	/// Every Setting the core understands.
	All,
	/// One named Setting.
	Key(SettingKey),
}

/// Settings resolved for one scope, fenced by the journal position the
/// snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSnapshot {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The scope the Settings were resolved for.
	pub scope: SettingScope,
	/// The resolved Settings in catalog order.
	pub settings: Vec<ResolvedSetting>,
}

impl SettingSelection {
	/// The keys this selection resolves.
	pub(crate) fn keys(self) -> Vec<SettingKey> {
		match self {
			Self::All => SettingKey::all(),
			Self::Key(key) => vec![key],
		}
	}
}

/// Resolves `keys` for `scope` from the rows the scope chain stores,
/// narrowest scope winning and the built-in default beneath them all.
///
/// A row this core cannot read—an unknown key, or a value a later release
/// reshaped—resolves as if the scope stored nothing, so an older `jetd`
/// still answers from the scope above it (ADR-0073).
pub(crate) fn resolve(
	keys: &[SettingKey],
	stored: &[SettingRecord],
) -> Vec<ResolvedSetting> {
	keys.iter()
		.map(|&key| {
			let winner = stored
				.iter()
				.filter(|record| record.key == key.as_str())
				.filter_map(|record| {
					let scope = SettingScope::from_record(record.scope);
					let value = SettingValue::decode(&record.value)?;
					Some((scope, value))
				})
				.max_by_key(|(stored_scope, _)| stored_scope.kind());
			match winner {
				Some((stored_scope, value)) => ResolvedSetting {
					key,
					value,
					source: SettingSource::Scope(stored_scope),
				},
				None => ResolvedSetting {
					key,
					value: key.built_in(),
					source: SettingSource::BuiltIn,
				},
			}
		})
		.collect()
}

/// Validates a Setting write and encodes its value for the store.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the scope may not store
/// the Setting or the value does not fit it.
pub(crate) fn prepare_write(
	key: SettingKey,
	scope: SettingScope,
	value: &SettingValue,
) -> Result<String, CoreError> {
	key.require_scope(scope)?;
	key.require_value(value)?;
	value.encode()
}

/// Validates removing whatever a scope stores for a Setting.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the scope may not store
/// the Setting.
pub(crate) fn prepare_clear(
	key: SettingKey,
	scope: SettingScope,
) -> Result<(), CoreError> {
	key.require_scope(scope)
}

/// What the journal files a change to `scope` under. A Conversation's
/// Settings belong to its history; the Plane's and a Project's belong to
/// the Plane.
pub(crate) fn event_subject(scope: SettingScope) -> EventSubject {
	match scope {
		SettingScope::Plane | SettingScope::Project { .. } => {
			EventSubject::Plane
		}
		SettingScope::Conversation { conversation_id } => {
			EventSubject::Conversation(conversation_id)
		}
	}
}

/// The value the Plane itself resolves for `key`: whatever the Plane scope
/// stores, and the built-in default until something does.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the values cannot be read.
pub(crate) async fn resolve_plane(
	tx: &mut ReadTransaction,
	key: SettingKey,
) -> Result<SettingValue, CoreError> {
	let stored = tx.settings_for_scope(SettingScope::Plane.record()).await?;
	Ok(resolve(&[key], &stored)
		.into_iter()
		.next()
		.map_or_else(|| key.built_in(), |resolved| resolved.value))
}

/// Refuses a scope whose subject this Plane does not have.
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when the named Project or
/// Conversation does not exist, or a store category when the check cannot
/// be answered.
pub(crate) async fn require_subject(
	tx: &mut ReadTransaction,
	scope: SettingScope,
) -> Result<(), CoreError> {
	match scope {
		SettingScope::Plane => Ok(()),
		SettingScope::Project { project_id } => {
			if tx.project(project_id.0).await?.is_some() {
				Ok(())
			} else {
				Err(CoreError::not_found(
					"project.not_found",
					"the Project does not exist",
				))
			}
		}
		SettingScope::Conversation { conversation_id } => {
			if tx.conversation(conversation_id.0).await?.is_some() {
				Ok(())
			} else {
				Err(CoreError::not_found(
					"conversation.not_found",
					"the Conversation does not exist",
				))
			}
		}
	}
}
