//! Conversation identities, provenance, retention, and pagination records.

use super::{WorkingTreeRecord, column_error};
use crate::StoreError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Whether Jet keeps a Conversation after its final Run (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
	/// Keep the Conversation and its history indefinitely. The default.
	Retain,
	/// Forget the Conversation once it has no active Run and no other
	/// protected state.
	ForgetAfterFinalRun,
}

impl RetentionPolicy {
	/// The durable spelling, also used in JSON.
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Retain => "retain",
			Self::ForgetAfterFinalRun => "forget_after_final_run",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[Self::Retain, Self::ForgetAfterFinalRun]
			.into_iter()
			.find(|retention| retention.as_str() == text)
	}
}

/// Where a Conversation came from (ADR-0010, ADR-0035).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationOriginRecord {
	/// Created in Jet.
	New,
	/// Created to continue an Imported conversation, recorded in
	/// `imported_conversations`.
	Imported {
		/// The import it continues.
		import_id: Uuid,
	},
	/// Created from one immutable Change checkpoint.
	Forked {
		/// Conversation that owns the selected Run.
		source_conversation_id: Uuid,
		/// Run that owns the selected checkpoint.
		source_run_id: Uuid,
		/// One-based turn boundary selected from that Run.
		checkpoint_turn: u32,
	},
}

/// Storage columns whose valid combinations spell one Conversation origin.
pub(crate) struct ConversationOriginColumns {
	pub(crate) import_id: Option<Uuid>,
	pub(crate) fork_source_conversation_id: Option<Uuid>,
	pub(crate) fork_source_run_id: Option<Uuid>,
	pub(crate) fork_checkpoint_turn: Option<i64>,
}

impl ConversationOriginRecord {
	/// Columns whose valid combinations spell one origin.
	pub(crate) fn columns(self) -> ConversationOriginColumns {
		match self {
			Self::New => ConversationOriginColumns {
				import_id: None,
				fork_source_conversation_id: None,
				fork_source_run_id: None,
				fork_checkpoint_turn: None,
			},
			Self::Imported { import_id } => ConversationOriginColumns {
				import_id: Some(import_id),
				fork_source_conversation_id: None,
				fork_source_run_id: None,
				fork_checkpoint_turn: None,
			},
			Self::Forked {
				source_conversation_id,
				source_run_id,
				checkpoint_turn,
			} => ConversationOriginColumns {
				import_id: None,
				fork_source_conversation_id: Some(source_conversation_id),
				fork_source_run_id: Some(source_run_id),
				fork_checkpoint_turn: Some(i64::from(checkpoint_turn)),
			},
		}
	}

	pub(crate) fn parse(
		columns: ConversationOriginColumns,
	) -> Result<Self, StoreError> {
		let ConversationOriginColumns {
			import_id,
			fork_source_conversation_id,
			fork_source_run_id,
			fork_checkpoint_turn,
		} = columns;
		match (
			import_id,
			fork_source_conversation_id,
			fork_source_run_id,
			fork_checkpoint_turn,
		) {
			(None, None, None, None) => Ok(Self::New),
			(Some(import_id), None, None, None) => {
				Ok(Self::Imported { import_id })
			}
			(
				None,
				Some(source_conversation_id),
				Some(source_run_id),
				Some(turn),
			) => {
				let checkpoint_turn = u32::try_from(turn).map_err(|_| {
					column_error(
						"fork_checkpoint_turn",
						format!("invalid checkpoint turn {turn}"),
					)
				})?;
				if checkpoint_turn == 0 {
					return Err(column_error(
						"fork_checkpoint_turn",
						"a checkpoint turn is positive".into(),
					));
				}
				Ok(Self::Forked {
					source_conversation_id,
					source_run_id,
					checkpoint_turn,
				})
			}
			combination => Err(column_error(
				"conversation_origin",
				format!("invalid origin columns {combination:?}"),
			)),
		}
	}
}

/// A Conversation to insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewConversation {
	/// Globally unique identity chosen by the caller.
	pub conversation_id: Uuid,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// Where the Conversation does its work.
	pub working_tree: WorkingTreeRecord,
	/// Where the Conversation came from.
	pub origin: ConversationOriginRecord,
	/// When the caller recorded the Conversation.
	pub created_at_unix_ms: i64,
}

/// Current state of one Conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRecord {
	/// Globally unique identity.
	pub conversation_id: Uuid,
	/// Current version for conflict-sensitive Conversation Commands.
	pub revision: u64,
	/// Retention choice.
	pub retention: RetentionPolicy,
	/// Where the Conversation does its work.
	pub working_tree: WorkingTreeRecord,
	/// Where the Conversation came from.
	pub origin: ConversationOriginRecord,
	/// Resolved user-facing name and its authority.
	pub name: NameRecord,
	/// When the Conversation was recorded.
	pub created_at_unix_ms: i64,
}

/// Authority that supplied a current Conversation or Run name (ADR-0044).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSourceRecord {
	/// An interactive user's authoritative choice.
	Manual,
	/// A validated Utility-model result. Its full attribution belongs to the
	/// Utility result record introduced with Utility execution (ADR-0099).
	Utility,
	/// A structured title supplied by the Harness through its Craft.
	HarnessNative,
	/// Stable local text used when no stronger source is available.
	Deterministic,
}

impl NameSourceRecord {
	/// Stable database spelling.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Manual => "manual",
			Self::Utility => "utility",
			Self::HarnessNative => "harness_native",
			Self::Deterministic => "deterministic",
		}
	}

	pub(crate) fn parse(value: &str) -> Option<Self> {
		match value {
			"manual" => Some(Self::Manual),
			"utility" => Some(Self::Utility),
			"harness_native" => Some(Self::HarnessNative),
			"deterministic" => Some(Self::Deterministic),
			_ => None,
		}
	}
}

/// Resolved, unescaped display text beside the source that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRecord {
	/// Original validated text. Rendering contexts encode it at output time.
	pub value: String,
	/// Authority that supplied `value`.
	pub source: NameSourceRecord,
}

pub(crate) fn parse_name(
	value: Option<String>,
	source: Option<&str>,
	fallback: NameRecord,
) -> Result<NameRecord, crate::StoreError> {
	match (value, source) {
		(Some(value), Some(source)) => Ok(NameRecord {
			value,
			source: NameSourceRecord::parse(source).ok_or_else(|| {
				column_error(
					"name_source",
					format!("unknown name source {source:?}"),
				)
			})?,
		}),
		(None, None) => Ok(fallback),
		_ => Err(column_error(
			"name",
			"name and name_source must either both be present or both be absent"
				.into(),
		)),
	}
}

/// Opaque-to-callers key for continuing a Conversation keyset page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationPageKey(pub(crate) i64);

/// Where a Conversation page begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationPageStart {
	/// Read the first page of the snapshot.
	First,
	/// Continue strictly after a key returned by the previous page.
	After(ConversationPageKey),
}
