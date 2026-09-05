//! Wire form of mutable Settings (ADR-0085).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a Setting value lives. A Command writes exactly the scope it
/// names; a Query resolves the Plane's values and then the values of the
/// scope it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub enum SettingKey {
	/// Whether the Utility model names Conversations automatically.
	#[serde(rename = "utility.automatic_naming")]
	UtilityAutomaticNaming,
	/// Whether Jet commits Harness changes without being asked.
	#[serde(rename = "git.auto_commit")]
	GitAutoCommit,
	/// Plane-wide guidance for generated commit messages and pull-request
	/// text.
	#[serde(rename = "git.message_instructions")]
	GitMessageInstructions,
}

/// One Setting's value. Each key holds exactly one of these shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
	/// A yes-or-no choice.
	Flag(bool),
	/// Bounded free text.
	Text(String),
}

/// Which Settings one Query resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct SettingSnapshot {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The scope the Settings were resolved for.
	pub scope: SettingScope,
	/// The resolved Settings.
	pub settings: Vec<ResolvedSetting>,
}
