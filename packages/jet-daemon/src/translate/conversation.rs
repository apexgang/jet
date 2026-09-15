//! Conversation and Run snapshots. The Working tree and Workspace they
//! carry are translated in `workspace`.

use super::{
	audit, import, name, store_recovery, unix_ms,
	workspace::{working_tree, workspace},
};
use jet_core::{
	Conversation, ConversationList, ConversationOrigin, ConversationSnapshot,
	PlaneStatus, RetentionPolicy, Run, RunLifecycle,
};
use jet_protocol as wire;

pub(super) fn plane_status(
	status: &PlaneStatus,
	minor: u32,
) -> wire::PlaneStatus {
	wire::PlaneStatus {
		cursor: (minor >= wire::FENCED_READS_MINOR).then_some(status.cursor.0),
		plane_id: status.plane_id.0,
		daemon_starts: status.daemon_starts,
		started_at_unix_ms: unix_ms(status.started_at),
		core_version: status.core_version.into(),
		// A client that negotiated an older minor does not name the
		// Security audit, so it is not told about its state either
		// (ADR-0019).
		security: (minor >= wire::SECURITY_AUDIT_MINOR)
			.then(|| audit::security(status.security))
			.flatten(),
		recovery: (minor >= wire::STORE_RECOVERY_MINOR)
			.then(|| store_recovery::recovery_status(&status.recovery, minor)),
	}
}

pub(super) fn conversation_list(
	list: &ConversationList,
	minor: u32,
) -> wire::ConversationList {
	wire::ConversationList {
		cursor: list.cursor.0,
		conversations: list
			.conversations
			.iter()
			.map(|conversation| self::conversation(conversation, minor))
			.collect(),
		next_page: list
			.next_page
			.as_ref()
			.map(|cursor| wire::PageCursor(cursor.0)),
	}
}

pub(super) fn conversation_snapshot(
	snapshot: &ConversationSnapshot,
	minor: u32,
) -> wire::ConversationSnapshot {
	wire::ConversationSnapshot {
		cursor: snapshot.cursor.0,
		conversation: conversation(&snapshot.conversation, minor),
		// A client that negotiated an older minor is not told about a
		// Workspace it cannot decode (ADR-0019).
		workspace: (minor >= wire::WORKSPACES_MINOR)
			.then(|| {
				snapshot
					.workspace
					.as_ref()
					.map(|value| workspace(value, minor))
			})
			.flatten(),
		runs: snapshot
			.runs
			.iter()
			.map(|value| run(value, minor))
			.collect(),
	}
}

pub(super) fn conversation(
	conversation: &Conversation,
	minor: u32,
) -> wire::Conversation {
	wire::Conversation {
		conversation_id: conversation.conversation_id.0,
		revision: (minor >= wire::NAMES_MINOR)
			.then_some(conversation.revision.0),
		retention: retention(conversation.retention),
		working_tree: (minor >= wire::WORKSPACES_MINOR)
			.then(|| working_tree(conversation.working_tree)),
		// A client that negotiated an older minor does not know where a
		// Conversation can come from, so it is not told (ADR-0019).
		origin: match conversation.origin {
			ConversationOrigin::Forked { .. }
				if minor < wire::CONVERSATION_FORKS_MINOR =>
			{
				None
			}
			_ if minor >= wire::IMPORTED_CONVERSATIONS_MINOR => {
				Some(import::origin(conversation.origin))
			}
			_ => None,
		},
		name: (minor >= wire::NAMES_MINOR)
			.then(|| name::resolved(&conversation.name)),
		created_at_unix_ms: unix_ms(conversation.created_at),
	}
}

pub(super) fn run(run: &Run, minor: u32) -> wire::Run {
	wire::Run {
		run_id: run.run_id.0,
		conversation_id: run.conversation_id.0,
		revision: run.revision.0,
		lifecycle: lifecycle(run.lifecycle),
		name: (minor >= wire::NAMES_MINOR).then(|| name::resolved(&run.name)),
		created_at_unix_ms: unix_ms(run.created_at),
		ended_at_unix_ms: run.ended_at.map(unix_ms),
	}
}

pub(super) fn retention(retention: RetentionPolicy) -> wire::RetentionPolicy {
	match retention {
		RetentionPolicy::Retain => wire::RetentionPolicy::Retain,
		RetentionPolicy::ForgetAfterFinalRun => {
			wire::RetentionPolicy::ForgetAfterFinalRun
		}
	}
}

pub(super) fn retention_from_wire(
	retention: wire::RetentionPolicy,
) -> RetentionPolicy {
	match retention {
		wire::RetentionPolicy::Retain => RetentionPolicy::Retain,
		wire::RetentionPolicy::ForgetAfterFinalRun => {
			RetentionPolicy::ForgetAfterFinalRun
		}
	}
}

pub(super) fn lifecycle(lifecycle: RunLifecycle) -> wire::RunLifecycle {
	match lifecycle {
		RunLifecycle::Created => wire::RunLifecycle::Created,
		RunLifecycle::Starting => wire::RunLifecycle::Starting,
		RunLifecycle::Active => wire::RunLifecycle::Active,
		RunLifecycle::Stopping => wire::RunLifecycle::Stopping,
		RunLifecycle::Completed => wire::RunLifecycle::Completed,
		RunLifecycle::Failed => wire::RunLifecycle::Failed,
		RunLifecycle::Canceled => wire::RunLifecycle::Canceled,
		RunLifecycle::Lost => wire::RunLifecycle::Lost,
	}
}

pub(super) fn lifecycle_from_wire(
	lifecycle: wire::RunLifecycle,
) -> RunLifecycle {
	match lifecycle {
		wire::RunLifecycle::Created => RunLifecycle::Created,
		wire::RunLifecycle::Starting => RunLifecycle::Starting,
		wire::RunLifecycle::Active => RunLifecycle::Active,
		wire::RunLifecycle::Stopping => RunLifecycle::Stopping,
		wire::RunLifecycle::Completed => RunLifecycle::Completed,
		wire::RunLifecycle::Failed => RunLifecycle::Failed,
		wire::RunLifecycle::Canceled => RunLifecycle::Canceled,
		wire::RunLifecycle::Lost => RunLifecycle::Lost,
	}
}
