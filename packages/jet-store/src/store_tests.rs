use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{
	ActorRecord, CommandReceiptRecord, ConversationRecord,
	EVENT_COMPACTION_BATCH_LIMIT, EventClass, EventRecord, NewCommandReceipt,
	NewConversation, NewEvent, NewRun, PlaneRecord, RetentionPolicy,
	RunLifecycle, RunRecord, SettingRecord, SettingScopeRecord, Store,
	StoreError, WorkingTreeRecord, is_unavailable_code,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

#[tokio::test]
async fn plane_identity_and_start_count_survive_reopening_the_store() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");

	let first = Store::open(&path).await.unwrap();
	let after_first_start = first.record_daemon_start().await.unwrap();
	drop(first);

	let second = Store::open(&path).await.unwrap();
	let after_second_start = second.record_daemon_start().await.unwrap();

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
	assert_eq!(second.plane().await.unwrap(), after_second_start);
}

#[tokio::test]
async fn a_fresh_store_has_a_plane_that_never_started_a_daemon() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();

	let plane = store.plane().await.unwrap();
	assert_eq!(plane.daemon_starts, 0);
	assert!(!plane.plane_id.is_nil());
}

#[tokio::test]
async fn opening_a_store_in_a_missing_directory_is_reported_as_unavailable() {
	let dir = tempfile::tempdir().unwrap();
	let error = Store::open(&dir.path().join("missing").join("plane.sqlite3"))
		.await
		.unwrap_err();

	assert!(matches!(error, StoreError::Unavailable(_)), "{error:?}");
}

/// SQLite reports transient lock and I/O trouble through extended result
/// codes whose low byte carries the primary code, so the mapping has to mask
/// before it decides that the store is merely unreachable.
#[test]
fn extended_result_codes_keep_the_meaning_of_the_code_they_extend() {
	let unavailable = [5, 261, 517, 773, 6, 262, 518, 10, 266, 778, 14, 270];
	let integrity = [1, 8, 11, 19, 26, 275, 787, 1299, 1555, 2067];

	assert_eq!(
		(
			unavailable.map(is_unavailable_code),
			integrity.map(is_unavailable_code)
		),
		([true; 12], [false; 10])
	);
}

/// ADR-0057 asks for one pinned SQLite build with WAL and FTS5. Linking a
/// distribution's SQLite instead is silent, and a build without FTS5 would
/// only surface once Plane-local search is written, so the capabilities
/// answer for themselves here.
#[tokio::test]
async fn the_linked_sqlite_build_offers_wal_and_fts5() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();

	sqlx::query("CREATE VIRTUAL TABLE fts5_probe USING fts5(body)")
		.execute(&store.pool)
		.await
		.expect("the linked SQLite build must provide FTS5");
	let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
		.fetch_one(&store.pool)
		.await
		.unwrap();

	assert_eq!(journal_mode, "wal");
}

/// ADR-0073 keeps releases rollback-compatible, so a `jetd` that predates a
/// migration still opens the store a newer release migrated, skipping the
/// version it does not know rather than refusing the whole store.
#[tokio::test]
async fn a_store_migrated_by_a_newer_release_still_opens() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();
	sqlx::query(
		"INSERT INTO _sqlx_migrations (version, description, success,
			checksum, execution_time)
		 VALUES (99990101000000, 'written by a newer release', TRUE, X'00', 0)",
	)
	.execute(&store.pool)
	.await
	.unwrap();
	drop(store);

	let reopened = Store::open(&path).await.unwrap();

	assert_eq!(reopened.plane().await.unwrap().daemon_starts, 0);
}

/// Closing the store lets SQLite checkpoint its write-ahead log and remove
/// it, so the next open has nothing to replay.
#[tokio::test]
async fn closing_the_store_checkpoints_the_write_ahead_log() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let log = path.with_extension("sqlite3-wal");
	let store = Store::open(&path).await.unwrap();
	store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id: Uuid::now_v7(),
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await
		})
		.await
		.unwrap();
	let while_serving = log.exists();

	store.close().await;

	assert_eq!((while_serving, log.exists()), (true, false));
}

/// SQLite ends a transaction by itself when a statement fails on a full
/// disk, which desynchronizes the driver's transaction counter and makes the
/// rollback that follows fail. A connection returned to the pool in that
/// state would refuse every later transaction, so it must not be reused.
#[tokio::test]
async fn a_connection_left_mid_transaction_is_replaced_rather_than_reused() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();

	let wedged = store
		.write(async |tx| {
			// Stands in for the rollback SQLite performs on its own: the
			// transaction ends without the driver being told.
			sqlx::query("ROLLBACK").execute(tx.connection()).await?;
			Err::<(), _>(StoreError::Integrity("the disk filled up".into()))
		})
		.await
		.unwrap_err();

	let recorded = store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await
		})
		.await
		.unwrap();

	assert!(matches!(wedged, StoreError::Integrity(_)), "{wedged:?}");
	assert_eq!(
		recorded,
		ConversationRecord {
			conversation_id,
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTreeRecord::NoProject,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	);
}

/// A `jetd` from before the schema tracker moved into the driver leaves an
/// empty `schema_migrations` behind on any store it opens, including one
/// this release wrote. That leftover must not condemn a healthy store.
#[tokio::test]
async fn a_current_store_survives_being_opened_by_a_pre_release_jetd() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();
	sqlx::query(
		"CREATE TABLE schema_migrations (
			version INTEGER PRIMARY KEY,
			applied_at_unix_ms INTEGER NOT NULL
		)",
	)
	.execute(&store.pool)
	.await
	.unwrap();
	drop(store);

	let reopened = Store::open(&path).await.unwrap();

	assert_eq!(reopened.plane().await.unwrap().daemon_starts, 0);
}

/// A store the pre-release tracker still owns has nothing the migrator can
/// build on, so it is refused with an instruction rather than left to fail
/// on the first `CREATE TABLE`.
#[tokio::test]
async fn a_pre_release_store_is_refused_before_it_is_migrated() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let store = Store::open(&path).await.unwrap();
	// The shape the previous release left: its own tracker and none of the
	// driver's.
	for statement in [
		"DROP TABLE _sqlx_migrations",
		"CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY)",
	] {
		sqlx::query(statement).execute(&store.pool).await.unwrap();
	}
	drop(store);

	let error = Store::open(&path).await.unwrap_err();

	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
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

#[tokio::test]
async fn operational_event_compaction_is_bounded_and_preserves_cursor_truth() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();
	let event_count = EVENT_COMPACTION_BATCH_LIMIT + 2;
	store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			tx.append_event(conversation_event(
				conversation_id,
				"conversation.created",
			))
			.await?;
			for index in 0..event_count {
				tx.append_event(operational_event(
					conversation_id,
					index as i64,
				))
				.await?;
			}
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();

	let compact = async || {
		store
			.write(async |tx| {
				let coverage = tx.verified_projection_coverage().await?;
				tx.compact_operational_events(coverage, i64::MAX).await
			})
			.await
			.unwrap()
	};
	let first = compact().await;
	let expired = store
		.read(async |tx| tx.events_after(0, 10).await)
		.await
		.unwrap_err();
	let second = compact().await;

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

#[tokio::test]
async fn compaction_stops_before_operational_events_inside_the_grace_period() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();
	store
		.write(async |tx| {
			tx.append_event(operational_event(conversation_id, 1))
				.await?;
			tx.append_event(operational_event(conversation_id, 3))
				.await?;
			tx.append_event(operational_event(conversation_id, 1))
				.await?;
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();

	let removed = store
		.write(async |tx| {
			let coverage = tx.verified_projection_coverage().await?;
			tx.compact_operational_events(coverage, 2).await
		})
		.await
		.unwrap();
	let (cursor, events) = store
		.read(async |tx| tx.events_after(1, 10).await)
		.await
		.unwrap();

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

/// Compaction is the one statement that reuses a numbered parameter: it
/// mentions the coverage bound three times and the grace cutoff twice. The
/// two are interchangeable on the data the other compaction tests use, so
/// this one is shaped so that binding them the other way round removes a
/// different number of Events.
#[tokio::test]
async fn compaction_binds_the_coverage_bound_apart_from_the_grace_cutoff() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();
	store
		.write(async |tx| {
			for _ in 0..3 {
				tx.append_event(operational_event(conversation_id, 1))
					.await?;
			}
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();

	// Coverage 3 with cutoff 2 removes all three; swapping them would bound
	// the sequence by 2 and remove only two.
	let removed = store
		.write(async |tx| {
			let coverage = tx.verified_projection_coverage().await?;
			tx.compact_operational_events(coverage, 2).await
		})
		.await
		.unwrap();

	assert_eq!(removed, 3);
}

#[tokio::test]
async fn snapshot_coverage_cannot_compact_another_plane() {
	let dir = tempfile::tempdir().unwrap();
	let first = Store::open(&dir.path().join("first.sqlite3"))
		.await
		.unwrap();
	let second = Store::open(&dir.path().join("second.sqlite3"))
		.await
		.unwrap();
	let coverage = first
		.write(async |tx| {
			tx.append_event(operational_event(Uuid::now_v7(), 1))
				.await?;
			tx.verified_projection_coverage().await
		})
		.await
		.unwrap();
	second
		.write(async |tx| {
			tx.append_event(operational_event(Uuid::now_v7(), 1))
				.await?;
			Ok::<_, StoreError>(())
		})
		.await
		.unwrap();

	let error = second
		.write(async |tx| tx.compact_operational_events(coverage, 2).await)
		.await
		.unwrap_err();

	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
}

#[tokio::test]
async fn a_cursor_ahead_of_the_journal_is_rejected() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();

	let error = store
		.read(async |tx| tx.events_after(1, 10).await)
		.await
		.unwrap_err();

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

#[tokio::test]
async fn conversations_runs_and_events_survive_reopening_the_store() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let conversation_id = Uuid::now_v7();
	let run_id = Uuid::now_v7();

	let first = Store::open(&path).await.unwrap();
	let (conversation, run, created, ended) = first
		.write(async |tx| {
			let conversation = tx
				.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
			assert_eq!(tx.runs(conversation_id).await?, vec![]);
			let created = tx
				.append_event(conversation_event(
					conversation_id,
					"conversation.created",
				))
				.await?;
			tx.insert_run(NewRun {
				run_id,
				conversation_id,
				created_at_unix_ms: NOW_UNIX_MS + 1,
			})
			.await?;
			let run = tx
				.update_run_lifecycle(
					run_id,
					RunLifecycle::Completed,
					NOW_UNIX_MS + 2,
				)
				.await?;
			let ended = tx
				.append_event(run_event(
					conversation_id,
					run_id,
					"run.lifecycle_changed",
				))
				.await?;
			Ok::<_, StoreError>((conversation, run, created, ended))
		})
		.await
		.unwrap();
	drop(first);

	let second = Store::open(&path).await.unwrap();
	let (conversations, runs, events, cursor) = second
		.read(async |tx| {
			Ok::<_, StoreError>((
				tx.conversations().await?,
				tx.runs(conversation_id).await?,
				tx.events_after(0, 10).await?,
				tx.event_cursor().await?,
			))
		})
		.await
		.unwrap();
	let later = second
		.write(async |tx| {
			tx.append_event(conversation_event(conversation_id, "later"))
				.await
		})
		.await
		.unwrap();

	assert_eq!(
		conversation,
		ConversationRecord {
			conversation_id,
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTreeRecord::NoProject,
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

#[tokio::test]
async fn a_failed_write_leaves_no_trace_of_its_changes() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();

	let error = store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::ForgetAfterFinalRun,
				working_tree: WorkingTreeRecord::NoProject,
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await?;
			tx.append_event(conversation_event(
				conversation_id,
				"conversation.created",
			))
			.await?;
			Err::<(), _>(StoreError::Integrity("rejected by the caller".into()))
		})
		.await
		.unwrap_err();

	let (conversation, cursor) = store
		.read(async |tx| {
			Ok::<_, StoreError>((
				tx.conversation(conversation_id).await?,
				tx.event_cursor().await?,
			))
		})
		.await
		.unwrap();
	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
	assert_eq!((conversation, cursor), (None, 0));
}

#[tokio::test]
async fn a_run_cannot_be_recorded_for_an_unknown_conversation() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();

	let error = store
		.write(async |tx| {
			tx.insert_run(NewRun {
				run_id: Uuid::now_v7(),
				conversation_id: Uuid::now_v7(),
				created_at_unix_ms: NOW_UNIX_MS,
			})
			.await
		})
		.await
		.unwrap_err();

	assert!(matches!(error, StoreError::Integrity(_)), "{error:?}");
}

#[tokio::test]
async fn expired_command_receipts_keep_only_an_identity_tombstone() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let actor = actor();
	let command_id = Uuid::now_v7();
	store
		.write(async |tx| {
			tx.insert_command_receipt(&NewCommandReceipt {
				actor,
				command_id,
				request_digest: [7; 32],
				recorded_at_unix_ms: 10,
				outcome_version: 1,
				outcome: r#"{"Ok":{}}"#.into(),
			})
			.await?;
			tx.prune_command_receipts_before(11).await
		})
		.await
		.unwrap();

	let receipt = store
		.read(async |tx| tx.command_receipt(actor, command_id).await)
		.await
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

fn setting(key: &str, scope: SettingScopeRecord, value: &str) -> SettingRecord {
	SettingRecord {
		key: key.into(),
		scope,
		value: value.into(),
		updated_at_unix_ms: NOW_UNIX_MS,
	}
}

#[tokio::test]
async fn a_scope_reads_the_values_it_stores_beside_the_planes() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();
	let elsewhere = SettingScopeRecord::Conversation {
		conversation_id: Uuid::now_v7(),
	};
	let addressed = SettingScopeRecord::Conversation { conversation_id };
	store
		.write(async |tx| {
			tx.upsert_setting(&setting(
				"utility.automatic_naming",
				SettingScopeRecord::Plane,
				"true",
			))
			.await?;
			tx.upsert_setting(&setting(
				"utility.automatic_naming",
				addressed,
				"false",
			))
			.await?;
			tx.upsert_setting(&setting("git.auto_commit", elsewhere, "true"))
				.await
		})
		.await
		.unwrap();

	let chain = store
		.read(async |tx| tx.settings_for_scope(addressed).await)
		.await
		.unwrap();

	assert_eq!(
		chain,
		vec![
			setting("utility.automatic_naming", addressed, "false"),
			setting(
				"utility.automatic_naming",
				SettingScopeRecord::Plane,
				"true"
			),
		]
	);
}

#[tokio::test]
async fn writing_a_scope_replaces_only_the_value_that_scope_stored() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();
	let addressed = SettingScopeRecord::Conversation { conversation_id };
	let key = "utility.automatic_naming";

	let (replaced, cleared) = store
		.write(async |tx| {
			tx.upsert_setting(&setting(
				key,
				SettingScopeRecord::Plane,
				"false",
			))
			.await?;
			tx.upsert_setting(&setting(key, addressed, "false")).await?;
			tx.upsert_setting(&setting(key, addressed, "true")).await?;
			let replaced = tx.settings_for_scope(addressed).await?;
			tx.delete_setting(key, addressed).await?;
			let cleared = tx.settings_for_scope(addressed).await?;
			Ok::<_, StoreError>((replaced, cleared))
		})
		.await
		.unwrap();

	assert_eq!(
		(replaced, cleared),
		(
			vec![
				setting(key, addressed, "true"),
				setting(key, SettingScopeRecord::Plane, "false"),
			],
			vec![setting(key, SettingScopeRecord::Plane, "false")]
		)
	);
}
