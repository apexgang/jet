use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ActorRecord, CommandReceiptRecord, ConversationRecord, EventRecord,
	NewCommandReceipt, NewConversation, NewEvent, NewRun, PlaneRecord,
	Retention, RunLifecycle, RunRecord, Store, StoreError,
};

#[test]
fn plane_identity_and_start_count_survive_reopening_the_store() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let first = Store::open(&path).unwrap();
	let after_first_start = first.record_daemon_start().unwrap();
	drop(first);

	let second = Store::open(&path).unwrap();
	let after_second_start = second.record_daemon_start().unwrap();

	assert_eq!(after_first_start.daemon_starts, 1);
	assert_eq!(
		after_second_start,
		PlaneRecord {
			plane_id: after_first_start.plane_id,
			daemon_starts: 2,
		}
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

fn event(conversation_id: Uuid, run_id: Option<Uuid>, kind: &str) -> NewEvent {
	NewEvent {
		event_id: Uuid::now_v7(),
		actor: actor(),
		conversation_id: Some(conversation_id),
		run_id,
		kind: kind.into(),
		payload_version: 1,
		payload: "{}".into(),
	}
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
				retention: Retention::Retain,
			})?;
			assert_eq!(tx.runs(conversation_id)?, vec![]);
			let created = tx.append_event(event(
				conversation_id,
				None,
				"conversation.created",
			))?;
			tx.insert_run(NewRun {
				run_id,
				conversation_id,
			})?;
			let run =
				tx.update_run_lifecycle(run_id, RunLifecycle::Completed)?;
			let ended = tx.append_event(event(
				conversation_id,
				Some(run_id),
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
		.write(|tx| tx.append_event(event(conversation_id, None, "later")))
		.unwrap();

	assert_eq!(
		conversation,
		ConversationRecord {
			conversation_id,
			retention: Retention::Retain,
			created_at_unix_ms: conversation.created_at_unix_ms,
		}
	);
	assert_eq!(
		run,
		RunRecord {
			run_id,
			conversation_id,
			revision: 2,
			lifecycle: RunLifecycle::Completed,
			created_at_unix_ms: run.created_at_unix_ms,
			ended_at_unix_ms: run.ended_at_unix_ms,
		}
	);
	assert!(run.ended_at_unix_ms.is_some());
	assert_eq!(conversations, vec![conversation]);
	assert_eq!(runs, vec![run]);
	assert_eq!(
		events,
		vec![
			EventRecord {
				sequence: 1,
				..created
			},
			EventRecord {
				sequence: 2,
				..ended
			}
		]
	);
	assert_eq!(cursor, 2);
	assert_eq!(later.sequence, 3);
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
				retention: Retention::ForgetAfterFinalRun,
			})?;
			tx.append_event(event(
				conversation_id,
				None,
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
