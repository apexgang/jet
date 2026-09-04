use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ActorRecord, CommandReceiptRecord, ConversationRecord,
	EVENT_COMPACTION_BATCH_LIMIT, EventClass, EventRecord, NewCommandReceipt,
	NewConversation, NewEvent, NewRun, PlaneRecord, RetentionPolicy,
	RunLifecycle, RunRecord, Store, StoreError,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

#[test]
fn plane_identity_and_start_count_survive_reopening_the_store() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let first = Store::open(&path).unwrap();
	let after_first_start = first.record_daemon_start().unwrap();
	drop(first);

	let second = Store::open(&path).unwrap();
	let after_second_start = second.record_daemon_start().unwrap();

	assert_eq!(
		(after_first_start, &after_second_start),
		(
			PlaneRecord {
				plane_id: after_second_start.plane_id,
				daemon_starts: 1,
			},
			&PlaneRecord {
				plane_id: after_second_start.plane_id,
				daemon_starts: 2,
			}
		)
	);
	assert_eq!(second.plane().unwrap(), after_second_start);
}

#[test]
fn a_fresh_store_has_a_plane_that_never_started_a_daemon() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();

	let plane = store.plane().unwrap();
	assert_eq!(plane.daemon_starts, 0);
	assert!(!plane.plane_id.is_nil());
}

#[test]
fn opening_a_store_in_a_missing_directory_is_reported_as_unavailable() {
	let dir = tempfile::tempdir().unwrap();
	let error = Store::open(&dir.path().join("missing").join("plane.sqlite3"))
		.unwrap_err();

	assert!(matches!(error, StoreError::Unavailable(_)), "{error:?}");
}

fn actor() -> ActorRecord {
	ActorRecord::InteractiveClient {
		client_id: Uuid::nil(),
	}
}

fn conversation_event(conversation_id: Uuid, kind: &str) -> NewEvent {
	NewEvent {
		event_id: Uuid::now_v7(),
		actor: actor(),
		recorded_at_unix_ms: NOW_UNIX_MS,
		conversation_id: Some(conversation_id),
		run_id: None,
		kind: kind.into(),
		payload_version: 1,
		payload: "{}".into(),
		class: EventClass::Semantic,
	}
}

fn run_event(conversation_id: Uuid, run_id: Uuid, kind: &str) -> NewEvent {
	NewEvent {
		run_id: Some(run_id),
		..conversation_event(conversation_id, kind)
	}
}

fn operational_event(
	conversation_id: Uuid,
	recorded_at_unix_ms: i64,
) -> NewEvent {
	NewEvent {
		recorded_at_unix_ms,
		class: EventClass::Operational,
		..conversation_event(conversation_id, "run.output_progressed")
	}
}

#[test]
fn operational_event_compaction_is_bounded_and_preserves_cursor_truth() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();
	let conversation_id = Uuid::now_v7();
	let event_count = EVENT_COMPACTION_BATCH_LIMIT + 2;
	store
		.write(|tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				created_at_unix_ms: NOW_UNIX_MS,
			})?;
			tx.append_event(conversation_event(
				conversation_id,
				"conversation.created",
			))?;
			for index in 0..event_count {
				tx.append_event(operational_event(
					conversation_id,
					index as i64,
				))?;
			}
			Ok::<_, StoreError>(())
		})
		.unwrap();

	let compact = || {
		store
			.write(|tx| {
				let coverage = tx.verified_projection_coverage()?;
				tx.compact_operational_events(coverage, i64::MAX)
			})
			.unwrap()
	};
	let first = compact();
	let expired = store.read(|tx| tx.events_after(0, 10)).unwrap_err();
	let second = compact();

	assert_eq!((first, second), (EVENT_COMPACTION_BATCH_LIMIT, 2));
	let StoreError::CursorExpired {
		minimum_available_cursor,
		current_snapshot_revision,
	} = expired
	else {
		panic!("expected an expired cursor, got {expired:?}");
	};
	assert_eq!(
		(minimum_available_cursor, current_snapshot_revision),
		(
			u64::try_from(EVENT_COMPACTION_BATCH_LIMIT + 1).unwrap(),
			u64::try_from(event_count + 1).unwrap(),
		)
	);
}

#[test]
fn compaction_stops_before_operational_events_inside_the_grace_period() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();
	let conversation_id = Uuid::now_v7();
	store
		.write(|tx| {
			tx.append_event(operational_event(conversation_id, 1))?;
			tx.append_event(operational_event(conversation_id, 3))?;
			tx.append_event(operational_event(conversation_id, 1))?;
			Ok::<_, StoreError>(())
		})
		.unwrap();

	let removed = store
		.write(|tx| {
			let coverage = tx.verified_projection_coverage()?;
			tx.compact_operational_events(coverage, 2)
		})
		.unwrap();
	let (cursor, events) = store.read(|tx| tx.events_after(1, 10)).unwrap();

	assert_eq!(removed, 1);
	assert_eq!(cursor, 3);
	assert_eq!(
		events
			.into_iter()
			.map(|event| (event.sequence, event.recorded_at_unix_ms))
			.collect::<Vec<_>>(),
		vec![(2, 3), (3, 1)]
	);
}

#[test]
fn snapshot_coverage_cannot_compact_another_plane() {
	let dir = tempfile::tempdir().unwrap();
	let first = Store::open(&dir.path().join("first.sqlite3")).unwrap();
	let second = Store::open(&dir.path().join("second.sqlite3")).unwrap();
	let coverage = first
		.write(|tx| {
			tx.append_event(operational_event(Uuid::now_v7(), 1))?;
			tx.verified_projection_coverage()
		})
		.unwrap();
	second
		.write(|tx| {
			tx.append_event(operational_event(Uuid::now_v7(), 1))?;
			Ok::<_, StoreError>(())
		})
		.unwrap();

	let error = second
		.write(|tx| tx.compact_operational_events(coverage, 2))
		.unwrap_err();

	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
}

#[test]
fn a_cursor_ahead_of_the_journal_is_rejected() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();

	let error = store.read(|tx| tx.events_after(1, 10)).unwrap_err();

	assert!(
		matches!(
			error,
			StoreError::CursorAhead {
				current_snapshot_revision: 0
			}
		),
		"{error:?}"
	);
}

#[test]
fn conversations_runs_and_events_survive_reopening_the_store() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let conversation_id = Uuid::now_v7();
	let run_id = Uuid::now_v7();

	let first = Store::open(&path).unwrap();
	let (conversation, run, created, ended) = first
		.write(|tx| {
			let conversation = tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				created_at_unix_ms: NOW_UNIX_MS,
			})?;
			assert_eq!(tx.runs(conversation_id)?, vec![]);
			let created = tx.append_event(conversation_event(
				conversation_id,
				"conversation.created",
			))?;
			tx.insert_run(NewRun {
				run_id,
				conversation_id,
				created_at_unix_ms: NOW_UNIX_MS + 1,
			})?;
			let run = tx.update_run_lifecycle(
				run_id,
				RunLifecycle::Completed,
				NOW_UNIX_MS + 2,
			)?;
			let ended = tx.append_event(run_event(
				conversation_id,
				run_id,
				"run.lifecycle_changed",
			))?;
			Ok::<_, StoreError>((conversation, run, created, ended))
		})
		.unwrap();
	drop(first);

	let second = Store::open(&path).unwrap();
	let (conversations, runs, events, cursor) = second
		.read(|tx| {
			Ok::<_, StoreError>((
				tx.conversations()?,
				tx.runs(conversation_id)?,
				tx.events_after(0, 10)?,
				tx.event_cursor()?,
			))
		})
		.unwrap();
	let later = second
		.write(|tx| {
			tx.append_event(conversation_event(conversation_id, "later"))
		})
		.unwrap();

	assert_eq!(
		conversation,
		ConversationRecord {
			conversation_id,
			retention: RetentionPolicy::Retain,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	);
	assert_eq!(
		run,
		RunRecord {
			run_id,
			conversation_id,
			revision: 2,
			lifecycle: RunLifecycle::Completed,
			created_at_unix_ms: NOW_UNIX_MS + 1,
			ended_at_unix_ms: Some(NOW_UNIX_MS + 2),
		}
	);
	assert_eq!(conversations, vec![conversation]);
	assert_eq!(runs, vec![run]);
	assert_eq!(
		events,
		(
			2,
			vec![
				EventRecord {
					sequence: 1,
					..created
				},
				EventRecord {
					sequence: 2,
					..ended
				}
			],
		)
	);
	assert_eq!((cursor, later.sequence), (2, 3));
}

#[test]
fn a_failed_write_leaves_no_trace_of_its_changes() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();
	let conversation_id = Uuid::now_v7();

	let error = store
		.write(|tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::ForgetAfterFinalRun,
				created_at_unix_ms: NOW_UNIX_MS,
			})?;
			tx.append_event(conversation_event(
				conversation_id,
				"conversation.created",
			))?;
			Err::<(), _>(StoreError::Integrity("rejected by the caller".into()))
		})
		.unwrap_err();

	let (conversation, cursor) = store
		.read(|tx| {
			Ok::<_, StoreError>((
				tx.conversation(conversation_id)?,
				tx.event_cursor()?,
			))
		})
		.unwrap();
	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
	assert_eq!((conversation, cursor), (None, 0));
}

#[test]
fn a_run_cannot_be_recorded_for_an_unknown_conversation() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();

	let error = store
		.write(|tx| {
			tx.insert_run(NewRun {
				run_id: Uuid::now_v7(),
				conversation_id: Uuid::now_v7(),
				created_at_unix_ms: NOW_UNIX_MS,
			})
		})
		.unwrap_err();

	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
}

#[test]
fn expired_command_receipts_keep_only_an_identity_tombstone() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3")).unwrap();
	let actor = actor();
	let command_id = Uuid::now_v7();
	store
		.write(|tx| {
			tx.insert_command_receipt(&NewCommandReceipt {
				actor,
				command_id,
				request_digest: [7; 32],
				recorded_at_unix_ms: 10,
				outcome_version: 1,
				outcome: r#"{"Ok":{}}"#.into(),
			})?;
			tx.prune_command_receipts_before(11)
		})
		.unwrap();

	let receipt = store
		.read(|tx| tx.command_receipt(actor, command_id))
		.unwrap()
		.unwrap();

	assert_eq!(
		receipt,
		CommandReceiptRecord {
			actor,
			command_id,
			request_digest: None,
			recorded_at_unix_ms: 10,
			outcome_version: None,
			outcome: None,
		}
	);
}
