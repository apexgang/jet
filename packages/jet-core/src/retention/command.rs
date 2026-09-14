//! Forgetting, deleting everywhere, and restoring, as Commands (ADR-0011).

use super::{Protection, TrashEntry, TrashReason, WorkspaceState, grace_ms};
use crate::{
	Actor, CommandId, CommandOutcome, ConversationId, CoreError,
	audit::{self, AuditDecision, AuditSubject, Decision},
	event::{EventKind, EventSubject},
	run::execution_control::{self, RunControl},
};
use jet_store::{
	AuditActorRecord, RunLifecycle, TrashReasonRecord, TrashRecord,
	WriteTransaction,
};

/// Stages a Conversation in Jet Trash because its owner asked Jet to
/// forget it. Live work is refused, not stopped: a Run that should end is
/// stopped by its own Command first (ADR-0011). Dirty and unpushed
/// Workspace state, schedules, and Effects protect automatic forgetting
/// only; here they are the owner's to weigh, and the preview shows them.
pub(crate) async fn forget(
	tx: &mut WriteTransaction,
	actor: &Actor,
	conversation_id: ConversationId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let protections = require_unstaged(tx, conversation_id).await?;
	if super::protection::has_live_work(&protections) {
		return Err(CoreError::conflict(
			"retention.live_work",
			"the Conversation has a live Run or queued turns; stop and \
			 withdraw them first, or delete it everywhere",
		));
	}
	staged(tx, actor, conversation_id, TrashReason::ManualForget, now).await
}

/// Stages a Conversation for deletion everywhere: stops its active Run
/// through the same escalation a Stop Run request uses, cancels its
/// queued turns, and records that its native history goes with it when
/// the grace period ends (ADR-0011, ADR-0083).
pub(crate) async fn delete_everywhere(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	conversation_id: ConversationId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	require_unstaged(tx, conversation_id).await?;
	for run in tx.runs(conversation_id.0).await? {
		match run.lifecycle {
			RunLifecycle::Active | RunLifecycle::Stopping => {
				execution_control::record(
					tx,
					actor,
					command_id,
					crate::RunId(run.run_id),
					RunControl::StopRun,
					now,
				)
				.await?;
			}
			// A launch cannot be cancelled in flight (ADR-0067); it is
			// stopped once it is a process, or recovered if it never is.
			RunLifecycle::Created | RunLifecycle::Starting => {
				return Err(CoreError::conflict(
					"retention.run_starting",
					"a Run of the Conversation is still starting; wait for \
					 it to become active, then ask again",
				));
			}
			RunLifecycle::Completed
			| RunLifecycle::Failed
			| RunLifecycle::Canceled
			| RunLifecycle::Lost => {}
		}
	}
	let mut queue = crate::turn::queue::load(tx, conversation_id).await?;
	let mut kept = Vec::new();
	for mut entry in queue.entries {
		if entry.turn.state == crate::TurnState::Queued {
			entry.turn.state = crate::TurnState::Canceled;
			crate::turn::queue::changed(
				tx,
				actor,
				conversation_id,
				&entry,
				now,
			)
			.await?;
		} else {
			kept.push(entry);
		}
	}
	queue.entries = kept;
	crate::turn::queue::save(tx, conversation_id, &queue).await?;
	staged(
		tx,
		actor,
		conversation_id,
		TrashReason::DeleteEverywhere,
		now,
	)
	.await
}

/// Takes a Conversation back out of Jet Trash before its grace period
/// ends. Nothing was removed yet, so nothing needs to come back.
pub(crate) async fn restore(
	tx: &mut WriteTransaction,
	actor: &Actor,
	conversation_id: ConversationId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	// A Transfer tombstone is content whose authority left; the fence
	// keeps it from becoming a Home Plane again (ADR-0070).
	if tx
		.trash_entry(conversation_id.0)
		.await?
		.is_some_and(|entry| entry.reason == TrashReasonRecord::Transferred)
	{
		return Err(CoreError::conflict(
			"retention.transferred",
			"the Conversation's authority moved to another Plane; its \
			 tombstone cannot be restored here",
		));
	}
	if !tx.delete_trash(conversation_id.0).await? {
		return Err(CoreError::not_found(
			"retention.not_trashed",
			"the Conversation is not in Jet Trash",
		));
	}
	tx.append_event(
		EventKind::ConversationRestored { conversation_id }.to_record(
			actor,
			EventSubject::Conversation(conversation_id),
			now,
		)?,
	)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::ConversationRestored,
			AuditSubject::Conversation(conversation_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::ConversationRestored { conversation_id })
}

/// Who is staging a Conversation: a client through a Command, whose
/// journal Events and audit record carry its identity, or the retention
/// sweep on the Conversation's own policy, which the audit attributes to
/// `retention` and which writes no journal Event of its own.
#[derive(Debug, Clone, Copy)]
pub(crate) enum StagedBy<'a> {
	Client(&'a Actor),
	Retention,
}

/// Stages `conversation_id` with `reason` under the Plane's grace period
/// and records the decision beside it.
pub(crate) async fn stage(
	tx: &mut WriteTransaction,
	by: StagedBy<'_>,
	conversation_id: ConversationId,
	reason: TrashReason,
	now: i64,
) -> Result<TrashEntry, CoreError> {
	let record = TrashRecord {
		conversation_id: conversation_id.0,
		reason: reason.record(),
		trashed_at_unix_ms: now,
		expires_at_unix_ms: now.saturating_add(grace_ms(tx).await?),
	};
	tx.insert_trash(record).await?;
	let decision = Decision::succeeded(
		match reason {
			TrashReason::ManualForget
			| TrashReason::AutomaticForget
			| TrashReason::AutodeleteRule => AuditDecision::ConversationForgotten,
			TrashReason::DeleteEverywhere
			| TrashReason::AutodeleteEverywhere => {
				AuditDecision::ConversationDeletionAuthorized
			}
			// The Transfer tombstone is left by relinquishing a Plane
			// transfer, which records its own decision (ADR-0070).
			TrashReason::PlaneTransfer => {
				return Err(CoreError::internal(
					"retention.tombstone_not_staged",
					"a Transfer tombstone is not staged; it is left by \
					 relinquishing",
				));
			}
		},
		AuditSubject::Conversation(conversation_id),
	);
	match by {
		StagedBy::Client(actor) => {
			tx.append_event(
				EventKind::ConversationTrashed {
					conversation_id,
					reason,
					expires_at_unix_ms: record.expires_at_unix_ms,
				}
				.to_record(
					actor,
					EventSubject::Conversation(conversation_id),
					now,
				)?,
			)
			.await?;
			audit::record(tx, actor, decision, now).await?;
		}
		StagedBy::Retention => {
			audit::record_as(tx, AuditActorRecord::Retention, decision, now)
				.await?;
		}
	}
	Ok(TrashEntry::from(record))
}

/// A client's staging, as its Command's outcome.
async fn staged(
	tx: &mut WriteTransaction,
	actor: &Actor,
	conversation_id: ConversationId,
	reason: TrashReason,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	stage(tx, StagedBy::Client(actor), conversation_id, reason, now)
		.await
		.map(CommandOutcome::ConversationTrashed)
}

/// The store-side protections of a Conversation that exists and is not
/// staged yet. Its Workspace is not inspected: a Command refuses live
/// work only, and Git has no say in that.
async fn require_unstaged(
	tx: &mut WriteTransaction,
	conversation_id: ConversationId,
) -> Result<Vec<Protection>, CoreError> {
	let protections =
		super::protections(tx, conversation_id, WorkspaceState::default())
			.await?;
	if tx.trash_entry(conversation_id.0).await?.is_some() {
		return Err(CoreError::conflict(
			"retention.already_trashed",
			"the Conversation is already in Jet Trash",
		));
	}
	Ok(protections)
}

#[cfg(test)]
mod tests {
	use super::super::fixtures::{audit, conversation, preview, trash};
	use crate::{
		AuditActor, Command, CommandOutcome, EventKind, Protection,
		RetentionPreview, TrashEntry, TrashReason,
		test_support::{
			FixedProbe, ManualClock, actor, equipped, events, request,
			start_core_with,
		},
		workspace::WorkingTreeRequest,
	};
	use jet_store::RetentionPolicy;
	use pretty_assertions::assert_eq;
	use std::time::{Duration, UNIX_EPOCH};

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const GRACE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

	/// Forgetting stages the Conversation under the default grace period,
	/// once; restoring takes it back, once; and a live Run refuses
	/// forgetting rather than being stopped by it (ADR-0011, ADR-0015).
	#[tokio::test]
	async fn forgetting_stages_restores_and_refuses_live_work() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			ManualClock::at(UNIX_EPOCH + NOW),
			FixedProbe::new(equipped()),
		)
		.await;
		let id = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		let forget = || {
			request(Command::ForgetConversation {
				conversation_id: id,
			})
		};
		let restore = || {
			request(Command::RestoreConversation {
				conversation_id: id,
			})
		};
		let expected = TrashEntry {
			conversation_id: id,
			reason: TrashReason::ManualForget,
			trashed_at: UNIX_EPOCH + NOW,
			expires_at: UNIX_EPOCH + NOW + GRACE,
		};

		let staged = core.execute(&actor(), forget()).await.unwrap();
		let listed = trash(&core).await;
		let previewed = preview(&core, id).await;
		let twice = core.execute(&actor(), forget()).await.unwrap_err();
		let restored = core.execute(&actor(), restore()).await.unwrap();
		let emptied = trash(&core).await;
		let again = core.execute(&actor(), restore()).await.unwrap_err();
		core.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: id,
			}),
		)
		.await
		.unwrap();
		let live = core.execute(&actor(), forget()).await.unwrap_err();
		let protected = preview(&core, id).await;

		assert_eq!(
			(
				staged,
				listed,
				previewed,
				twice.code,
				restored,
				emptied,
				again.code,
				live.code,
				protected.protections,
				events(&core)
					.await
					.into_iter()
					.filter(|kind| matches!(
						kind,
						EventKind::ConversationTrashed { .. }
							| EventKind::ConversationRestored { .. }
					))
					.collect::<Vec<_>>(),
				audit(&core).await,
			),
			(
				CommandOutcome::ConversationTrashed(expected.clone()),
				vec![expected.clone()],
				RetentionPreview {
					conversation_id: id,
					protections: vec![],
					trash: Some(expected),
					audit_records: 1,
				},
				"retention.already_trashed".to_string(),
				CommandOutcome::ConversationRestored {
					conversation_id: id
				},
				vec![],
				"retention.not_trashed".to_string(),
				"retention.live_work".to_string(),
				vec![Protection::ActiveRun],
				vec![
					EventKind::ConversationTrashed {
						conversation_id: id,
						reason: TrashReason::ManualForget,
						expires_at_unix_ms: 1_700_000_000_000
							+ i64::try_from(GRACE.as_millis()).unwrap(),
					},
					EventKind::ConversationRestored {
						conversation_id: id
					},
				],
				vec![
					(
						"conversation.forgotten".into(),
						AuditActor::InteractiveClient {
							client_id: crate::ClientId(uuid::Uuid::nil()),
						},
						Some(id.0.to_string()),
					),
					(
						"conversation.restored".into(),
						AuditActor::InteractiveClient {
							client_id: crate::ClientId(uuid::Uuid::nil()),
						},
						Some(id.0.to_string()),
					),
				],
			)
		);
	}

	/// Deleting everywhere is refused while a Run is still launching, and
	/// otherwise stages with its own reason and a destructive audit
	/// decision (ADR-0011).
	#[tokio::test]
	async fn deleting_everywhere_waits_for_launches_and_records_its_reason() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			ManualClock::at(UNIX_EPOCH + NOW),
			FixedProbe::new(equipped()),
		)
		.await;
		let launching = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		core.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: launching,
			}),
		)
		.await
		.unwrap();
		let idle = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		let delete = |conversation_id| {
			request(Command::DeleteConversationEverywhere { conversation_id })
		};

		let refused =
			core.execute(&actor(), delete(launching)).await.unwrap_err();
		let staged = core.execute(&actor(), delete(idle)).await.unwrap();

		assert_eq!(
			(refused.code, staged, audit(&core).await.pop()),
			(
				"retention.run_starting".to_string(),
				CommandOutcome::ConversationTrashed(TrashEntry {
					conversation_id: idle,
					reason: TrashReason::DeleteEverywhere,
					trashed_at: UNIX_EPOCH + NOW,
					expires_at: UNIX_EPOCH + NOW + GRACE,
				}),
				Some((
					"conversation.deletion_authorized".into(),
					AuditActor::InteractiveClient {
						client_id: crate::ClientId(uuid::Uuid::nil()),
					},
					Some(idle.0.to_string()),
				)),
			)
		);
	}
}
