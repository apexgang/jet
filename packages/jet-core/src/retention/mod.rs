//! Retention, forgetting, and Jet Trash (ADR-0001, ADR-0011, ADR-0015).
//!
//! A Conversation is retained by default. Forgetting it removes what Jet
//! owns about it, its Runs, Workspace, journal, names, and Artifact
//! references, and leaves the Harness's own history where it is. Deleting
//! it everywhere additionally stops its active work and, after the same
//! grace period, asks the Harness for its native history too, where a
//! Craft can do that. Neither happens at once: both stage the Conversation
//! in Jet Trash with the reason recorded, and the deletion itself happens
//! when the grace period ends, unless the Conversation is restored first.
//!
//! Automatic forgetting is the retention policy `forget_after_final_run`
//! acting on its own. It stages a Conversation only when nothing protects
//! it: no live Run, no queued turn, no enabled schedule, no dirty or
//! unpushed Workspace, and no unresolved Effect. A pin in a Conversation
//! layout would protect it too, and will when layouts arrive (ADR-0034);
//! nothing in this core can be pinned yet.
//!
//! The deletion reaches the Deletion ledger before it commits, so a
//! restored snapshot cannot bring the Conversation back (ADR-0102), and
//! the Security audit keeps its records about the Conversation with the
//! identity replaced by the opaque reference it was chained over
//! (ADR-0105). The preview discloses how many records that is.

mod command;
mod preview;
mod protection;
mod sweep;
mod workspace_state;

pub(crate) use command::{delete_everywhere, forget, restore};
pub(crate) use protection::protections;
pub use sweep::RetentionSweep;
pub(crate) use sweep::Staging;
pub(crate) use workspace_state::WorkspaceState;

use crate::{ConversationId, EventSequence, setting::SettingKey};
use jet_store::{TrashReasonRecord, TrashRecord};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Days a Conversation stays in Jet Trash before it is deleted, unless a
/// Setting says otherwise (ADR-0015).
pub(crate) const DEFAULT_TRASH_GRACE_DAYS: u32 = 30;

/// The shortest grace period a Setting may choose. A day leaves time to
/// notice a staging and restore it.
pub(crate) const MINIMUM_TRASH_GRACE_DAYS: u32 = 1;

pub(crate) const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// One reason a Conversation is not eligible for automatic forgetting, or
/// for manual forgetting where the reason is live work (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
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

/// Why a Conversation is in Jet Trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
	/// An approved Autodelete rule matched it while nothing protected it
	/// (ADR-0015).
	AutodeleteRule,
	/// An approved Autodelete rule separately authorized to delete
	/// everywhere matched it (ADR-0011).
	AutodeleteEverywhere,
	/// Its Home Plane authority moved to another Plane; the content stays
	/// as the Transfer tombstone until the grace period ends (ADR-0070).
	PlaneTransfer,
}

impl TrashReason {
	pub(crate) fn record(self) -> TrashReasonRecord {
		match self {
			Self::ManualForget => TrashReasonRecord::Manual,
			Self::AutomaticForget => TrashReasonRecord::Automatic,
			Self::DeleteEverywhere => TrashReasonRecord::Everywhere,
			Self::AutodeleteRule => TrashReasonRecord::Autodelete,
			Self::AutodeleteEverywhere => {
				TrashReasonRecord::AutodeleteEverywhere
			}
			Self::PlaneTransfer => TrashReasonRecord::Transferred,
		}
	}

	fn from_record(record: TrashReasonRecord) -> Self {
		match record {
			TrashReasonRecord::Manual => Self::ManualForget,
			TrashReasonRecord::Automatic => Self::AutomaticForget,
			TrashReasonRecord::Everywhere => Self::DeleteEverywhere,
			TrashReasonRecord::Autodelete => Self::AutodeleteRule,
			TrashReasonRecord::AutodeleteEverywhere => {
				Self::AutodeleteEverywhere
			}
			TrashReasonRecord::Transferred => Self::PlaneTransfer,
		}
	}
}

/// One Conversation in Jet Trash: why it is there and how long it stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
	/// The Conversation staged for deletion.
	pub conversation_id: ConversationId,
	/// Why it was staged.
	pub reason: TrashReason,
	/// When it was staged.
	pub trashed_at: SystemTime,
	/// When its grace period ends and the deletion happens.
	pub expires_at: SystemTime,
}

impl From<TrashRecord> for TrashEntry {
	fn from(record: TrashRecord) -> Self {
		Self {
			conversation_id: ConversationId(record.conversation_id),
			reason: TrashReason::from_record(record.reason),
			trashed_at: crate::system_time(record.trashed_at_unix_ms),
			expires_at: crate::system_time(record.expires_at_unix_ms),
		}
	}
}

/// Everything in Jet Trash, soonest expiry first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTrash {
	/// Event cursor from the same read transaction.
	pub cursor: EventSequence,
	/// The staged Conversations, bounded to 256.
	pub entries: Vec<TrashEntry>,
}

/// What forgetting or deleting one Conversation would meet and leave
/// behind, read before the decision is made (ADR-0011, ADR-0105).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPreview {
	/// The Conversation asked about.
	pub conversation_id: ConversationId,
	/// What protects it today. Automatic forgetting waits for all of them;
	/// manual forgetting refuses only live work.
	pub protections: Vec<Protection>,
	/// Its Trash entry, if it is already staged.
	pub trash: Option<TrashEntry>,
	/// How many Security-audit records name it, which the deletion keeps
	/// as content-free metadata under an opaque identifier.
	pub audit_records: usize,
}

/// The grace period the Plane gives a staged Conversation.
pub(crate) async fn grace_ms(
	tx: &mut jet_store::ReadTransaction,
) -> Result<i64, crate::CoreError> {
	let value =
		crate::setting::resolve_plane(tx, SettingKey::RetentionTrashGraceDays)
			.await?;
	let crate::setting::SettingValue::Count(days) = value else {
		return Err(crate::CoreError::internal(
			"retention.grace_unreadable",
			format!("the Trash grace period resolved to {value:?}"),
		));
	};
	Ok(i64::from(days).saturating_mul(DAY_MS))
}

#[cfg(test)]
pub(crate) mod fixtures {
	//! What the retention tests share: Conversations, ended Runs, and the
	//! reads that show what became of them.

	use crate::{
		Command, CommandOutcome, ConversationId, Core, Query, QueryResult,
		RetentionPreview, TrashEntry,
		test_support::{actor, request},
		workspace::WorkingTreeRequest,
	};
	use jet_store::{RetentionPolicy, RunLifecycle};

	pub(crate) async fn conversation(
		core: &Core,
		retention: RetentionPolicy,
		working_tree: WorkingTreeRequest,
	) -> ConversationId {
		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention,
					working_tree,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Conversation");
		};
		conversation.conversation_id
	}

	/// Creates a Run and ends it at once, so the Conversation has had its
	/// final Run.
	pub(crate) async fn finished_run(
		core: &Core,
		conversation_id: ConversationId,
	) {
		let CommandOutcome::RunCreated(run) = core
			.execute(&actor(), request(Command::CreateRun { conversation_id }))
			.await
			.unwrap()
		else {
			panic!("expected a Run");
		};
		core.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: RunLifecycle::Canceled,
			}),
		)
		.await
		.unwrap();
	}

	pub(crate) async fn trash(core: &Core) -> Vec<TrashEntry> {
		let QueryResult::ConversationTrash(trash) = core
			.query(&actor(), Query::ConversationTrash)
			.await
			.unwrap()
		else {
			panic!("expected the Trash");
		};
		trash.entries
	}

	pub(crate) async fn preview(
		core: &Core,
		conversation_id: ConversationId,
	) -> RetentionPreview {
		let QueryResult::RetentionPreview(preview) = core
			.query(&actor(), Query::RetentionPreview { conversation_id })
			.await
			.unwrap()
		else {
			panic!("expected a preview");
		};
		preview
	}

	/// Every audit decision with who made it and what it still names.
	pub(crate) async fn audit(
		core: &Core,
	) -> Vec<(String, crate::AuditActor, Option<String>)> {
		let QueryResult::SecurityAudit(page) = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: crate::AuditSequence(0),
				},
			)
			.await
			.unwrap()
		else {
			panic!("expected an audit page");
		};
		page.entries
			.into_iter()
			.map(|entry| (entry.decision, entry.actor, entry.target.identity))
			.collect()
	}
}
