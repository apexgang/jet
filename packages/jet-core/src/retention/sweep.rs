//! The retention sweep: automatic forgetting into Jet Trash, and the
//! deletion of what has stayed there past its grace period (ADR-0015).

use super::{
	Protection, TrashReason, WorkspaceState,
	command::{StagedBy, stage},
	protections,
};
use crate::{
	ConversationId, Core, CoreError, RecoveryMode,
	audit::{self, AuditDecision, AuditSubject, Decision},
	security::SecurityClass,
};
use jet_store::{AuditActorRecord, SnapshotReason, TrashRecord};
use std::path::PathBuf;

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionSweep {
	/// Conversations staged in Jet Trash by their retention policy.
	pub trashed: Vec<ConversationId>,
	/// Conversations deleted for good after their grace period.
	pub deleted: Vec<ConversationId>,
}

impl Core {
	/// Stages every unprotected Conversation whose policy forgets it after
	/// its final Run, then deletes every staged Conversation whose grace
	/// period has ended. Nothing runs while the Plane is in Recovery mode
	/// or cannot vouch for its Security audit: forgetting is a decision the
	/// audit exists to record, and deletion is destructive (ADR-0105).
	///
	/// A Conversation whose Workspace cannot be inspected is left alone
	/// this time, as if protected. Deleting takes the day's Recovery
	/// snapshot first when none exists yet (ADR-0097), waits while the
	/// Conversation has live work again, reaches the Deletion ledger
	/// before it commits (ADR-0102), and removes the Workspace directory
	/// after; its Artifact payloads become unreferenced and collection
	/// takes them.
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when the store cannot be
	/// read or written, or the snapshot cannot be taken.
	pub async fn sweep_retention(&self) -> Result<RetentionSweep, CoreError> {
		let mut sweep = RetentionSweep::default();
		if self.recovery_mode() != RecoveryMode::Serving
			|| self
				.security
				.read()
				.await
				.admit(SecurityClass::Guarded)
				.is_err()
		{
			return Ok(sweep);
		}
		let now = self.now_unix_ms();
		let mut after = String::new();
		loop {
			let candidates = self
				.store
				.read(async |tx| tx.forgettable_conversations(&after).await)
				.await?;
			let Some(last) = candidates.last().copied() else {
				break;
			};
			after = last.to_string();
			for conversation_id in candidates.into_iter().map(ConversationId) {
				if self
					.stage_if_unprotected(
						conversation_id,
						Staging::Policy(TrashReason::AutomaticForget),
						now,
					)
					.await?
				{
					sweep.trashed.push(conversation_id);
				}
			}
		}
		let expired = self
			.store
			.read(async |tx| tx.expired_trash(now).await)
			.await?;
		if !expired.is_empty() {
			self.store
				.snapshot_if_due(SnapshotReason::Maintenance, now)
				.await?;
		}
		for entry in expired {
			if self.delete_for_good(entry, now).await? {
				sweep.deleted.push(ConversationId(entry.conversation_id));
			}
		}
		Ok(sweep)
	}

	/// Stages one candidate if nothing protects it and `staging`, asked
	/// again inside the staging transaction, still names a reason; says
	/// whether it did. The store is checked twice: once to decide, outside
	/// any lock, and again in the transaction that stages, so work
	/// admitted in between is not forgotten. The Autodelete sweep stages
	/// its matches through here too (ADR-0015).
	pub(crate) async fn stage_if_unprotected(
		&self,
		conversation_id: ConversationId,
		staging: Staging,
		now: i64,
	) -> Result<bool, CoreError> {
		let (unprotected, workspace) = self
			.store
			.read(async |tx| {
				let found =
					protections(tx, conversation_id, WorkspaceState::default())
						.await?;
				let workspace = tx.workspace_of(conversation_id.0).await?;
				Ok::<_, CoreError>((found.is_empty(), workspace))
			})
			.await?;
		if !unprotected {
			return Ok(false);
		}
		// Unreadable is protected: nothing is forgotten on a guess
		// (ADR-0015).
		let Ok(state) = WorkspaceState::of(workspace.as_ref()).await else {
			return Ok(false);
		};
		self.store
			.write(async |tx| {
				if !protections(tx, conversation_id, state).await?.is_empty()
					|| tx.trash_entry(conversation_id.0).await?.is_some()
				{
					return Ok(false);
				}
				let Some(reason) =
					staging.reason(tx, conversation_id, now).await?
				else {
					return Ok(false);
				};
				stage(tx, StagedBy::Retention, conversation_id, reason, now)
					.await?;
				Ok::<_, CoreError>(true)
			})
			.await
	}

	/// Deletes one expired entry and says whether it did: the rows, the
	/// ledger line, the audit record of the deletion, and then the
	/// identity in every audit record about the Conversation, in one
	/// transaction; the Workspace directory after it commits. Work
	/// admitted since the staging, a Run or a queued turn, or an Effect
	/// still in flight, holds the deletion until it ends; the entry stays
	/// expired and the next sweep asks again. A Conversation deleted
	/// everywhere would also have its native history requested from its
	/// Craft here; no bundled Craft offers that in this release, so the
	/// Harness's history stays.
	async fn delete_for_good(
		&self,
		entry: TrashRecord,
		now: i64,
	) -> Result<bool, CoreError> {
		let conversation_id = ConversationId(entry.conversation_id);
		let worktree = self
			.store
			.write(async |tx| {
				let found =
					protections(tx, conversation_id, WorkspaceState::default())
						.await?;
				if found.iter().any(|protection| {
					matches!(
						protection,
						Protection::ActiveRun
							| Protection::PendingTurn
							| Protection::UnresolvedEffect
					)
				}) {
					return Ok(None);
				}
				let worktree = match tx.workspace_of(conversation_id.0).await? {
					Some(workspace) => Some(Worktree {
						project_root: tx
							.project(workspace.project_id)
							.await?
							.map(|project| PathBuf::from(project.root)),
						root: PathBuf::from(workspace.root),
					}),
					None => None,
				};
				tx.delete_conversation(conversation_id.0, now).await?;
				audit::record_as(
					tx,
					AuditActorRecord::Retention,
					Decision::succeeded(
						AuditDecision::ConversationDeleted,
						AuditSubject::Conversation(conversation_id),
					),
					now,
				)
				.await?;
				audit::anonymize(
					tx,
					AuditSubject::Conversation(conversation_id),
				)
				.await?;
				Ok::<_, CoreError>(Some(worktree))
			})
			.await?;
		match worktree {
			Some(Some(worktree)) => {
				worktree.remove().await;
				Ok(true)
			}
			Some(None) => Ok(true),
			None => Ok(false),
		}
	}
}

/// Who is asking to stage, and so what decides the reason inside the
/// staging transaction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Staging {
	/// The Conversation's own retention policy: the reason is settled.
	Policy(TrashReason),
	/// An Autodelete rule, read again inside the transaction so an edit or
	/// deletion since the page was read is respected (ADR-0015).
	Autodelete(crate::AutodeleteRuleId),
}

impl Staging {
	async fn reason(
		self,
		tx: &mut jet_store::WriteTransaction,
		conversation_id: ConversationId,
		now: i64,
	) -> Result<Option<TrashReason>, CoreError> {
		match self {
			Self::Policy(reason) => Ok(Some(reason)),
			Self::Autodelete(rule_id) => {
				crate::autodelete::still_matches(
					tx,
					rule_id,
					conversation_id,
					now,
				)
				.await
			}
		}
	}
}

/// A Workspace directory the store no longer names, and the repository it
/// was a worktree of, if that Project is still registered.
struct Worktree {
	project_root: Option<PathBuf>,
	root: PathBuf,
}

impl Worktree {
	/// Removes the directory. The repository forgets the worktree when it
	/// still exists; the directory goes either way, and a failure leaves a
	/// directory nothing refers to.
	async fn remove(self) {
		if let Some(project_root) = &self.project_root
			&& let Some(path) = self.root.to_str()
		{
			crate::workspace::worktree::remove_forced(project_root, path).await;
		}
		let _ = tokio::fs::remove_dir_all(&self.root).await;
	}
}

#[cfg(test)]
mod tests {
	use super::super::fixtures::{
		audit, conversation, finished_run, preview, trash,
	};
	use crate::{
		AuditActor, Command, CoreError, Protection, Query, RetentionSweep,
		TrashReason,
		test_support::{
			FixedProbe, ManualClock, actor, equipped, git, register_repository,
			request, start_core_with,
		},
		workspace::{BaseSelection, WorkingTreeRequest, seed::SeedSelection},
	};
	use jet_store::{DeletedIdentityKind, DeletionLedger, RetentionPolicy};
	use pretty_assertions::assert_eq;
	use std::{
		sync::Arc,
		time::{Duration, UNIX_EPOCH},
	};

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const DAY: Duration = Duration::from_secs(24 * 60 * 60);

	async fn started(dir: &std::path::Path) -> (crate::Core, Arc<ManualClock>) {
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&dir.join("plane.sqlite3"),
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		(core, clock)
	}

	/// A Conversation that forgets itself waits for its first Run to end,
	/// is then staged by the sweep under its own attribution, and is
	/// deleted once the grace period passes: gone from the store, in the
	/// ledger, and named in the audit by nothing but its opaque reference
	/// (ADR-0001, ADR-0015, ADR-0102, ADR-0105).
	#[tokio::test]
	async fn the_policy_stages_after_the_final_run_and_the_grace_period_deletes()
	 {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = started(dir.path()).await;
		let id = conversation(
			&core,
			RetentionPolicy::ForgetAfterFinalRun,
			WorkingTreeRequest::NoProject,
		)
		.await;
		let retained = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		finished_run(&core, retained).await;

		let before_any_run = core.sweep_retention().await.unwrap();
		let crate::CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: id,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Run");
		};
		let while_live = core.sweep_retention().await.unwrap();
		core.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: jet_store::RunLifecycle::Canceled,
			}),
		)
		.await
		.unwrap();
		let staged = core.sweep_retention().await.unwrap();
		let entry = trash(&core).await;
		clock.advance(29 * DAY);
		let too_soon = core.sweep_retention().await.unwrap();
		clock.advance(DAY);
		let deleted = core.sweep_retention().await.unwrap();
		let again = core.sweep_retention().await.unwrap();
		let lookup = core
			.query(
				&actor(),
				Query::Conversation {
					conversation_id: id,
				},
			)
			.await
			.map(|_| ())
			.map_err(|error: CoreError| error.code);
		let DeletionLedger::Verified(ledger) =
			core.store.deletion_ledger().unwrap()
		else {
			panic!("ledger is corrupt");
		};

		assert_eq!(
			(
				before_any_run,
				while_live,
				staged.clone(),
				entry
					.iter()
					.map(|e| (e.conversation_id, e.reason))
					.collect::<Vec<_>>(),
				too_soon,
				deleted,
				again,
				lookup,
				ledger
					.iter()
					.map(|record| (record.kind, record.identity))
					.collect::<Vec<_>>(),
				trash(&core).await,
				audit(&core).await,
			),
			(
				RetentionSweep::default(),
				RetentionSweep::default(),
				RetentionSweep {
					trashed: vec![id],
					deleted: vec![],
				},
				vec![(id, TrashReason::AutomaticForget)],
				RetentionSweep::default(),
				RetentionSweep {
					trashed: vec![],
					deleted: vec![id],
				},
				RetentionSweep::default(),
				Err("conversation.not_found".into()),
				vec![(DeletedIdentityKind::Conversation, id.0)],
				vec![],
				vec![
					(
						"conversation.forgotten".into(),
						AuditActor::Retention,
						None
					),
					(
						"conversation.deleted".into(),
						AuditActor::Retention,
						None
					),
				],
			)
		);
	}

	/// A Run created after a manual forgetting holds the deletion past the
	/// grace period; once it ends, the next sweep deletes.
	#[tokio::test]
	async fn live_work_admitted_after_staging_holds_the_deletion() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = started(dir.path()).await;
		let id = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		core.execute(
			&actor(),
			request(Command::ForgetConversation {
				conversation_id: id,
			}),
		)
		.await
		.unwrap();
		let crate::CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: id,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Run");
		};
		clock.advance(31 * DAY);

		let held = core.sweep_retention().await.unwrap();
		let still_staged = trash(&core).await.len();
		core.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: jet_store::RunLifecycle::Canceled,
			}),
		)
		.await
		.unwrap();
		let deleted = core.sweep_retention().await.unwrap();

		assert_eq!(
			(held, still_staged, deleted),
			(
				RetentionSweep::default(),
				1,
				RetentionSweep {
					trashed: vec![],
					deleted: vec![id],
				},
			)
		);
	}

	/// A Workspace with uncommitted changes, then with a commit no remote
	/// holds, protects its Conversation; once clean and empty of work it is
	/// staged, and its directory goes with the deletion (ADR-0001).
	#[tokio::test]
	async fn workspace_changes_protect_until_they_are_gone() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = started(dir.path()).await;
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
		let id = conversation(
			&core,
			RetentionPolicy::ForgetAfterFinalRun,
			WorkingTreeRequest::Workspace {
				project_id,
				base: BaseSelection::Head,
				seed: SeedSelection::None,
			},
		)
		.await;
		finished_run(&core, id).await;
		let root = crate::test_support::conversation_snapshot(&core, id)
			.await
			.workspace
			.unwrap()
			.root;

		std::fs::write(root.join("scratch.txt"), "work in progress\n").unwrap();
		let dirty = preview(&core, id).await.protections;
		let while_dirty = core.sweep_retention().await.unwrap();
		git(&root, &["add", "-A"]);
		git(&root, &["commit", "-q", "-m", "Local only"]);
		let unpushed = preview(&core, id).await.protections;
		let while_unpushed = core.sweep_retention().await.unwrap();
		git(&root, &["reset", "-q", "--hard", "HEAD~1"]);
		let clean = preview(&core, id).await.protections;
		let staged = core.sweep_retention().await.unwrap();
		clock.advance(31 * DAY);
		let deleted = core.sweep_retention().await.unwrap();

		assert_eq!(
			(
				dirty,
				while_dirty,
				unpushed,
				while_unpushed,
				clean,
				staged,
				deleted,
				root.exists(),
			),
			(
				vec![Protection::DirtyWorkspace],
				RetentionSweep::default(),
				vec![Protection::UnpushedWork],
				RetentionSweep::default(),
				vec![],
				RetentionSweep {
					trashed: vec![id],
					deleted: vec![],
				},
				RetentionSweep {
					trashed: vec![],
					deleted: vec![id],
				},
				false,
			)
		);
	}
}
