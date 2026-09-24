//! The deep check of the live store, run while the Plane is idle
//! (ADR-0077).
//!
//! The store owes a check after any change and runs it a table at a time;
//! the core decides that the Plane is idle enough to begin one and notices
//! the Command that arrives while it runs. It happens on the maintenance
//! wakeups the daemon already has, which every Command and Effect commit
//! triggers, so it never wakes an idle Plane on its own (ADR-0055).

use super::RecoveryMode;
use crate::{Core, error::CoreError};
use jet_store::DeepCheck;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What one wakeup did about the deep check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepCheckOutcome {
	/// None is owed, or the Plane is in Recovery mode already.
	NotDue,
	/// One is owed, but a Command, Effect, or execution is active; it
	/// stays owed.
	Busy,
	/// A Command arrived between two tables; the check stays owed.
	Abandoned,
	/// Every table of the live store is sound.
	Passed,
	/// The store is damaged, and the Plane is in read-only Recovery mode
	/// with the store preserved as found.
	Damaged,
}

/// A Command in flight, for as long as this is held.
pub(crate) struct CommandInFlight<'a>(&'a AtomicUsize);

impl Drop for CommandInFlight<'_> {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::Relaxed);
	}
}

impl Core {
	/// Counts a Command as in flight until the guard is dropped. The count
	/// is advisory: it keeps a deep check from beginning, or continuing,
	/// while a Command is being executed.
	pub(crate) fn command_in_flight(&self) -> CommandInFlight<'_> {
		self.commands_in_flight.fetch_add(1, Ordering::Relaxed);
		CommandInFlight(&self.commands_in_flight)
	}

	fn commands_active(&self) -> bool {
		self.commands_in_flight.load(Ordering::Relaxed) > 0
	}

	/// Runs the deep check of the live store when one is owed and the
	/// Plane is idle: no Command in flight, no Effect pending or in
	/// flight, and no execution or terminal alive (ADR-0077). A Command
	/// that arrives between two tables abandons the check, which is then
	/// owed again; damage puts the Plane in read-only Recovery mode, the
	/// same as an open that fails its check, and the store is preserved
	/// as found until a verified snapshot is restored.
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when the store cannot say
	/// whether the Plane is idle, or the check cannot reach it. The check
	/// is owed again either way.
	pub async fn check_store_if_idle(
		&self,
	) -> Result<DeepCheckOutcome, CoreError> {
		let now_unix_ms = self.now_unix_ms();
		if self.recovery_mode() != RecoveryMode::Serving
			|| !self.store.deep_check_due(now_unix_ms)
		{
			return Ok(DeepCheckOutcome::NotDue);
		}
		let busy = self.commands_active()
			|| self
				.store
				.read(async |tx| {
					Ok::<_, CoreError>(
						!tx.active_execution_ids("").await?.is_empty()
							|| !tx.live_terminals("").await?.is_empty()
							|| tx.has_unresolved_effects().await?,
					)
				})
				.await?;
		if busy {
			return Ok(DeepCheckOutcome::Busy);
		}
		let checked = self
			.store
			.deep_check(now_unix_ms, || self.commands_active())
			.await?;
		Ok(match checked {
			DeepCheck::NotDue => DeepCheckOutcome::NotDue,
			DeepCheck::Passed => DeepCheckOutcome::Passed,
			DeepCheck::Abandoned => DeepCheckOutcome::Abandoned,
			DeepCheck::Damaged => {
				self.recovery
					.send_replace(RecoveryMode::of(&self.store.integrity()));
				DeepCheckOutcome::Damaged
			}
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Command, CommandOutcome, ErrorCategory, IntegrityFailureReason,
		PairingGate, Query, QueryResult, RetentionPolicy, RunLifecycle,
		WorkingTreeRequest,
		test_support::{
			FixedProbe, ManualClock, actor, equipped, request, start_core_with,
		},
	};
	use jet_store::{
		EffectKindRecord, EffectSafetyRecord, EventClass, NewEffect, NewEvent,
		RunExecutionRecord, SnapshotReason,
	};
	use pretty_assertions::assert_eq;
	use std::{
		io::{Seek as _, SeekFrom, Write as _},
		path::Path,
		sync::Arc,
		time::{Duration, UNIX_EPOCH},
	};
	use uuid::Uuid;

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const DAY: Duration = Duration::from_secs(24 * 60 * 60);

	async fn start(path: &Path, clock: &Arc<ManualClock>) -> Core {
		start_core_with(
			path,
			Arc::clone(clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await
	}

	async fn set_gate(core: &Core, gate: PairingGate) {
		core.execute(&actor(), request(Command::SetPairingGate { gate }))
			.await
			.unwrap();
	}

	/// Appends Events until the journal is most of the file, so the page
	/// in its middle is the journal's wherever the schema's pages fall.
	async fn fill_journal(core: &Core) {
		core.store
			.write(async |tx| {
				for _ in 0..256 {
					tx.append_event(NewEvent {
						event_id: Uuid::now_v7(),
						actor: actor().record(),
						recorded_at_unix_ms: 0,
						conversation_id: None,
						run_id: None,
						kind: "run.progress".into(),
						payload_version: 1,
						payload: format!(
							"{{\"p\":\"{}\"}}",
							"x".repeat(16 * 1024)
						),
						class: EventClass::Operational,
					})
					.await?;
				}
				Ok::<(), jet_store::StoreError>(())
			})
			.await
			.unwrap();
	}

	/// Overwrites the page in the middle of a closed database, which the
	/// journal fills once [`fill_journal`] has run: a page nothing reads
	/// on the way to serving the store.
	fn damage_journal_page(path: &Path) {
		let length = std::fs::metadata(path).unwrap().len();
		let mut file =
			std::fs::OpenOptions::new().write(true).open(path).unwrap();
		file.seek(SeekFrom::Start(length / 2 / 4096 * 4096))
			.unwrap();
		file.write_all(&[0xff; 4096]).unwrap();
		file.sync_all().unwrap();
	}

	/// A started core owes one check and passes it; a Command the same
	/// day owes no second one, and one the next day does (ADR-0077).
	#[tokio::test]
	async fn an_idle_plane_checks_its_store_once_a_day() {
		let dir = tempfile::tempdir().unwrap();
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&dir.path().join("plane.sqlite3"), &clock).await;
		assert_eq!(
			(
				core.check_store_if_idle().await.unwrap(),
				core.check_store_if_idle().await.unwrap(),
			),
			(DeepCheckOutcome::Passed, DeepCheckOutcome::NotDue)
		);
		set_gate(&core, PairingGate::Closed).await;
		assert_eq!(
			core.check_store_if_idle().await.unwrap(),
			DeepCheckOutcome::NotDue
		);
		clock.advance(DAY);
		assert_eq!(
			(
				core.check_store_if_idle().await.unwrap(),
				core.recovery_mode(),
			),
			(DeepCheckOutcome::Passed, RecoveryMode::Serving)
		);
	}

	/// An owed check waits while a Command is in flight, an Effect is
	/// unresolved, or an execution is alive, and stays owed.
	#[tokio::test]
	async fn an_owed_check_waits_for_the_plane_to_be_idle() {
		let dir = tempfile::tempdir().unwrap();
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&dir.path().join("plane.sqlite3"), &clock).await;
		let in_flight = core.command_in_flight();
		assert_eq!(
			core.check_store_if_idle().await.unwrap(),
			DeepCheckOutcome::Busy
		);
		drop(in_flight);

		let effect_id = Uuid::now_v7();
		core.store
			.write(async |tx| {
				tx.insert_effect(&NewEffect {
					effect_id,
					command_id: Uuid::now_v7(),
					run_id: None,
					promotion_id: None,
					terminal_id: None,
					kind: EffectKindRecord::Utility,
					safety: EffectSafetyRecord::ReadOnly { max_attempts: 1 },
				})
				.await
			})
			.await
			.unwrap();
		assert_eq!(
			core.check_store_if_idle().await.unwrap(),
			DeepCheckOutcome::Busy
		);
		// In flight is as active as pending; finished, it is not.
		core.store
			.write(async |tx| tx.begin_effect_attempt(effect_id).await)
			.await
			.unwrap();
		assert_eq!(
			core.check_store_if_idle().await.unwrap(),
			DeepCheckOutcome::Busy
		);
		core.store
			.write(async |tx| {
				tx.finish_effect(
					effect_id,
					jet_store::EffectStateRecord::Completed,
				)
				.await
			})
			.await
			.unwrap();

		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::NoProject,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Conversation");
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: conversation.conversation_id,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Run");
		};
		let now = core.now_unix_ms();
		core.store
			.write(async |tx| {
				tx.insert_run_execution(
					run.run_id.0,
					&RunExecutionRecord {
						plan: "{}".into(),
						state: "{}".into(),
					},
				)
				.await?;
				tx.update_run_lifecycle(
					run.run_id.0,
					RunLifecycle::Starting,
					now,
				)
				.await
			})
			.await
			.unwrap();
		assert_eq!(
			core.check_store_if_idle().await.unwrap(),
			DeepCheckOutcome::Busy
		);
		core.store
			.write(async |tx| {
				tx.update_run_lifecycle(run.run_id.0, RunLifecycle::Lost, now)
					.await
			})
			.await
			.unwrap();
		assert_eq!(
			core.check_store_if_idle().await.unwrap(),
			DeepCheckOutcome::Passed
		);
	}

	/// Damage the open did not meet puts the Plane in read-only Recovery
	/// mode when the idle check finds it: Commands are refused, the status
	/// says why, the store is left as found, and restoring a verified
	/// snapshot is the way out (ADR-0077).
	#[tokio::test]
	async fn damage_found_while_idle_enters_read_only_recovery_mode() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start(&path, &clock).await;
		fill_journal(&core).await;
		// The Command indexes what was appended, so the restart below
		// has no reason to read those pages before the check does.
		set_gate(&core, PairingGate::Closed).await;
		let snapshot = core.snapshot_if_due().await.unwrap().unwrap();
		assert_eq!(snapshot.reason, SnapshotReason::Daily);
		core.close().await;
		drop(core);
		damage_journal_page(&path);
		let damaged_bytes = std::fs::read(&path).unwrap();

		let core = start(&path, &clock).await;
		assert_eq!(core.recovery_mode(), RecoveryMode::Serving);
		assert_eq!(
			(
				core.check_store_if_idle().await.unwrap(),
				core.recovery_mode(),
				core.check_store_if_idle().await.unwrap(),
			),
			(
				DeepCheckOutcome::Damaged,
				RecoveryMode::ReadOnly(IntegrityFailureReason::IntegrityCheck),
				DeepCheckOutcome::NotDue,
			)
		);
		let refused = core
			.execute(
				&actor(),
				request(Command::SetPairingGate {
					gate: PairingGate::Open,
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(
			(refused.category, refused.code.as_str(), refused.retryable),
			(ErrorCategory::Unavailable, "recovery.read_only", true)
		);
		let QueryResult::Status(status) =
			core.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("expected a status snapshot");
		};
		assert_eq!(
			status.recovery.mode,
			RecoveryMode::ReadOnly(IntegrityFailureReason::IntegrityCheck)
		);
		assert_eq!(std::fs::read(&path).unwrap(), damaged_bytes);

		let restored = core
			.execute(
				&actor(),
				request(Command::RestoreRecoverySnapshot {
					snapshot: snapshot.name.clone(),
				}),
			)
			.await
			.unwrap();
		let CommandOutcome::RecoverySnapshotRestored(restored) = restored
		else {
			panic!("expected a restoration");
		};
		assert_eq!(restored.snapshot, snapshot.name);
		assert!(restored.replaced.starts_with("plane.sqlite3.damaged-"));
		assert_eq!(core.recovery_mode(), RecoveryMode::Serving);
		set_gate(&core, PairingGate::Open).await;
	}
}
