//! Retention, forgetting, and Jet Trash (ADR-0011, ADR-0015). Introduced
//! by protocol minor 39.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Why a Conversation is in Jet Trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TrashReason {
	/// Its owner asked Jet to forget it.
	ManualForget,
	/// Its retention policy forgets it after its final Run, and nothing
	/// protected it any longer.
	AutomaticForget,
	/// Its owner asked for it to be deleted everywhere, native history
	/// included where the Harness supports that.
	DeleteEverywhere,
	/// An approved Autodelete rule matched it while nothing protected it.
	/// Needs minor 40.
	AutodeleteRule,
	/// An approved Autodelete rule separately authorized to delete
	/// everywhere matched it. Needs minor 40.
	AutodeleteEverywhere,
}

/// One Conversation in Jet Trash: why it is there and how long it stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TrashEntry {
	/// The Conversation staged for deletion.
	pub conversation_id: Uuid,
	/// Why it was staged.
	pub reason: TrashReason,
	/// When it was staged, in signed Unix milliseconds.
	pub trashed_at_unix_ms: i64,
	/// When its grace period ends and the deletion happens.
	pub expires_at_unix_ms: i64,
}

/// Everything in Jet Trash, soonest expiry first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConversationTrash {
	/// Plane Event high-water cursor in this read transaction.
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// The staged Conversations, bounded to 256.
	pub entries: Vec<TrashEntry>,
}

/// One reason a Conversation is not eligible for automatic forgetting.
/// Manual forgetting refuses only the first two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RetentionProtection {
	/// A Run of the Conversation has not ended.
	ActiveRun,
	/// A turn is queued or claimed and not yet settled.
	PendingTurn,
	/// An enabled Scheduled task will queue more turns.
	EnabledSchedule,
	/// Its Workspace has changes that are not committed.
	DirtyWorkspace,
	/// Its Workspace has commits no remote branch holds.
	UnpushedWork,
	/// An Effect of one of its Runs or deliveries is pending or in flight.
	UnresolvedEffect,
}

/// What forgetting or deleting one Conversation would meet and leave
/// behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RetentionPreview {
	/// The Conversation asked about.
	pub conversation_id: Uuid,
	/// What protects it today, in a fixed order.
	pub protections: Vec<RetentionProtection>,
	/// Its Trash entry, if it is already staged.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub trash: Option<TrashEntry>,
	/// How many Security-audit records name it. The deletion keeps them as
	/// content-free metadata under an opaque identifier until they expire.
	pub audit_records: u64,
}
