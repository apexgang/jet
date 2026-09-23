use std::time::Duration;

use jet_protocol::{
    AuditBreach, ClientMessage, CommandRequest, CommandResponse, DeletionLedgerStatus,
    ErrorCategory, PlaneStatus, QueryRequest, QueryResponse, RecoveryReason, RecoverySnapshot,
    RecoveryState as StoreState, RecoveryStatus, SecurityState, ServerMessage, SnapshotReason,
    StreamId, WireError,
};
use serde_json::json;
use tokio::net::{
    unix::{OwnedReadHalf, OwnedWriteHalf},
    UnixListener,
};
use uuid::Uuid;

use super::*;
use crate::jet::{
    client::{
        unit_tests::{accept, next_message, reply, request_id},
        PlaneClient,
    },
    enrollment::tests::{setup, Setup},
    planes::{PlaneBinding, PlaneId},
};

type Reader = jet_protocol::FrameReader<OwnedReadHalf>;
type Writer = jet_protocol::FrameWriter<OwnedWriteHalf>;

const CLIENT: Uuid = Uuid::from_u128(0x5ec0);
const IDENTITY: Uuid = Uuid::from_u128(0xfeed);
const DAILY: &str = "plane-1700000000000-daily.sqlite3";
const MIGRATION: &str = "plane-1690000000000-migration-40.sqlite3";

fn snapshot(name: &str, taken_at: i64, reason: SnapshotReason) -> RecoverySnapshot {
    RecoverySnapshot {
        name: name.into(),
        taken_at_unix_ms: taken_at,
        reason,
        bytes: 4096,
    }
}

fn listed() -> Vec<RecoverySnapshot> {
    vec![
        snapshot(DAILY, 1_700_000_000_000, SnapshotReason::Daily),
        snapshot(MIGRATION, 1_690_000_000_000, SnapshotReason::Migration),
    ]
}

fn status(
    state: StoreState,
    snapshots: Vec<RecoverySnapshot>,
    ledger: Option<DeletionLedgerStatus>,
    security: Option<SecurityState>,
) -> PlaneStatus {
    PlaneStatus {
        cursor: Some(9),
        plane_id: IDENTITY,
        daemon_starts: 3,
        started_at_unix_ms: 1_700_000_000_000,
        core_version: "1.43.0".into(),
        security,
        recovery: Some(RecoveryStatus {
            state,
            reason: (state == StoreState::ReadOnly).then_some(RecoveryReason::IntegrityCheckFailed),
            snapshots,
            deletion_ledger: ledger,
        }),
    }
}

fn read_only() -> PlaneStatus {
    status(
        StoreState::ReadOnly,
        listed(),
        Some(DeletionLedgerStatus::Verified { deletions: 2 }),
        None,
    )
}

fn serving() -> PlaneStatus {
    status(
        StoreState::Serving,
        listed(),
        Some(DeletionLedgerStatus::Verified { deletions: 2 }),
        Some(SecurityState::Trusted),
    )
}

// ---------------------------------------------------------------------------
// Snapshot tokens
// ---------------------------------------------------------------------------

#[test]
fn a_token_is_reused_while_its_name_is_listed_and_never_across_plane_identities() {
    let state = RecoveryState::default();
    let first = state.refresh(PlaneId::Local, IDENTITY, &listed()).unwrap();
    assert_eq!(
        serde_json::to_value(&first[0]).unwrap(),
        json!({
            "snapshotId": first[0].snapshot_id,
            "takenAtUnixMs": "1700000000000",
            "reason": "daily",
            "bytes": "4096",
        })
    );
    let daily = Uuid::parse_str(&first[0].snapshot_id).unwrap();
    let migration = Uuid::parse_str(&first[1].snapshot_id).unwrap();

    // A newer snapshot appears; the listed ones keep their tokens.
    let mut newer = vec![snapshot(
        "plane-1700100000000-daily.sqlite3",
        1_700_100_000_000,
        SnapshotReason::Daily,
    )];
    newer.extend(listed());
    let second = state.refresh(PlaneId::Local, IDENTITY, &newer).unwrap();
    assert_eq!(second[1].snapshot_id, daily.to_string());
    assert_eq!(second[2].snapshot_id, migration.to_string());

    // Rotated out: its token names nothing any more.
    state
        .refresh(PlaneId::Local, IDENTITY, &listed()[..1])
        .unwrap();
    assert_eq!(
        state.name(PlaneId::Local, IDENTITY, migration).unwrap(),
        None
    );
    assert_eq!(
        state
            .name(PlaneId::Local, IDENTITY, daily)
            .unwrap()
            .as_deref(),
        Some(DAILY)
    );
    // Another Plane identity at the same handle: nothing carries over.
    let other = Uuid::from_u128(0xbeef);
    assert_eq!(state.name(PlaneId::Local, other, daily).unwrap(), None);
    let moved = state.refresh(PlaneId::Local, other, &listed()).unwrap();
    assert_ne!(moved[0].snapshot_id, daily.to_string());
}

#[test]
fn listing_is_bounded_and_a_repeated_name_is_one_snapshot() {
    let state = RecoveryState::default();
    let many: Vec<RecoverySnapshot> = (0..100)
        .map(|index| {
            snapshot(
                &format!("plane-{index}-daily.sqlite3"),
                index,
                SnapshotReason::Daily,
            )
        })
        .collect();
    let views = state.refresh(PlaneId::Local, IDENTITY, &many).unwrap();
    assert_eq!(views.len(), MAX_LISTED_SNAPSHOTS);
    let repeated = vec![
        snapshot(DAILY, 1, SnapshotReason::Daily),
        snapshot(DAILY, 1, SnapshotReason::Daily),
    ];
    assert_eq!(
        state
            .refresh(PlaneId::Local, IDENTITY, &repeated)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reasons_map_and_only_plain_replaced_names_are_shown() {
    assert_eq!(reason_name(SnapshotReason::Daily), "daily");
    assert_eq!(reason_name(SnapshotReason::Migration), "migration");
    assert_eq!(reason_name(SnapshotReason::Maintenance), "maintenance");
    assert_eq!(
        replaced_name("plane.sqlite3.damaged-1757000100000").as_deref(),
        Some("plane.sqlite3.damaged-1757000100000")
    );
    for unsafe_name in [
        "",
        "../plane.sqlite3",
        "/home/user/.jet/plane.sqlite3.damaged-1",
        ".hidden",
        "name with space",
        &"a".repeat(MAX_REPLACED_NAME + 1),
    ] {
        assert_eq!(replaced_name(unsafe_name), None, "{unsafe_name}");
    }
}

#[test]
fn actions_accept_only_their_two_shapes() {
    let restore: RecoveryAction =
        serde_json::from_value(json!({"kind": "restore_snapshot", "snapshot_id": "x"})).unwrap();
    assert_eq!(
        restore,
        RecoveryAction::RestoreSnapshot {
            snapshot_id: "x".into()
        }
    );
    let purge: RecoveryAction = serde_json::from_value(json!({"kind": "purge_snapshots"})).unwrap();
    assert_eq!(purge, RecoveryAction::PurgeSnapshots {});
    for rejected in [
        json!({"kind": "restore_snapshot", "snapshot_id": "x", "name": DAILY}),
        json!({"kind": "restore_snapshot", "snapshot": DAILY}),
        json!({"kind": "purge_snapshots", "all": true}),
        json!({"kind": "begin_audit_epoch"}),
    ] {
        assert!(
            serde_json::from_value::<RecoveryAction>(rejected.clone()).is_err(),
            "{rejected}"
        );
    }
}

// ---------------------------------------------------------------------------
// Fake jetd
// ---------------------------------------------------------------------------

struct Fake {
    setup: Setup,
    plane_id: String,
    plane: PlaneId,
    listener: UnixListener,
}

fn fake_plane() -> Fake {
    let setup = setup();
    let socket = setup.directory.path().join("recovery-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0x5ec1);
    setup.bridge.planes.insert_for_test(
        id,
        "Build box",
        PlaneClient::new(
            socket,
            CLIENT,
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        ),
    );
    Fake {
        setup,
        plane_id: id.to_string(),
        plane: PlaneId::Remote(id),
        listener,
    }
}

impl Fake {
    fn bridge(&self) -> &JetBridge {
        &self.setup.bridge
    }

    fn state(&self) -> &RecoveryState {
        &self.bridge().system.recovery
    }

    /// Tokens as a health read of `snapshots` would have issued them.
    fn tokens(&self, snapshots: &[RecoverySnapshot]) -> Vec<String> {
        self.state()
            .refresh(self.plane, IDENTITY, snapshots)
            .unwrap()
            .into_iter()
            .map(|view| view.snapshot_id)
            .collect()
    }

    /// Prepares `action` against a Plane that reports `status`.
    async fn prepare(
        &self,
        action: RecoveryAction,
        status: PlaneStatus,
    ) -> Result<RecoveryReviewView, PublicError> {
        let (view, ()) = tokio::join!(prepare(self.bridge(), &self.plane_id, action), async {
            let (mut reader, mut writer) = accept(&self.listener, CLIENT).await;
            serve_status(&mut reader, &mut writer, status).await;
        });
        view
    }

    async fn execute(&self, review_id: &str) -> Result<RecoveryOutcome, PublicError> {
        execute(self.bridge(), &self.plane_id, review_id).await
    }

    /// No connection reached the Plane.
    async fn assert_untouched(&self) {
        let pending = tokio::time::timeout(Duration::from_millis(20), self.listener.accept()).await;
        assert!(pending.is_err(), "the Plane was contacted");
    }
}

async fn serve_status(reader: &mut Reader, writer: &mut Writer, status: PlaneStatus) {
    let (stream, message) = next_message(reader).await;
    assert!(
        matches!(
            message,
            ClientMessage::Query {
                query: QueryRequest::Status,
                ..
            }
        ),
        "expected a status read, got {message:?}"
    );
    reply(
        writer,
        stream,
        ServerMessage::QueryResult {
            id: request_id(&message),
            result: QueryResponse::Status(status),
        },
    )
    .await;
}

async fn command_result(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    result: CommandResponse,
) {
    reply(
        writer,
        stream,
        ServerMessage::CommandResult {
            id: request_id(message),
            result,
        },
    )
    .await;
}

async fn refuse(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    category: ErrorCategory,
    code: &str,
) {
    reply(
        writer,
        stream,
        ServerMessage::Error {
            id: Some(request_id(message)),
            error: WireError {
                category,
                code: code.into(),
                retryable: false,
                message: "daemon text never crosses".into(),
                revision_conflict: None,
                restart: None,
                recovery_actions: Vec::new(),
            },
        },
    )
    .await;
}

/// The next request must be the reviewed restore under `command_id`.
async fn expect_restore(reader: &mut Reader, command_id: Uuid) -> (StreamId, ClientMessage) {
    let (stream, message) = next_message(reader).await;
    match &message {
        ClientMessage::Command {
            command_id: sent,
            command: CommandRequest::RestoreRecoverySnapshot { snapshot },
            ..
        } if *sent == command_id && snapshot == DAILY => {}
        other => panic!("unexpected request {other:?}"),
    }
    (stream, message)
}

async fn expect_purge(reader: &mut Reader, command_id: Uuid) -> (StreamId, ClientMessage) {
    let (stream, message) = next_message(reader).await;
    match &message {
        ClientMessage::Command {
            command_id: sent,
            command: CommandRequest::PurgeRecoverySnapshots,
            ..
        } if *sent == command_id => {}
        other => panic!("unexpected request {other:?}"),
    }
    (stream, message)
}

fn review_id(view: &RecoveryReviewView) -> String {
    match view {
        RecoveryReviewView::RestoreSnapshot { review_id, .. }
        | RecoveryReviewView::PurgeSnapshots { review_id, .. } => review_id.clone(),
    }
}

fn restore_of(token: &str) -> RecoveryAction {
    RecoveryAction::RestoreSnapshot {
        snapshot_id: token.to_owned(),
    }
}

fn code(result: Result<RecoveryReviewView, PublicError>) -> (String, &'static str) {
    let error = result.unwrap_err();
    (error.code, error.category)
}

fn refused_code(outcome: &RecoveryOutcome) -> &str {
    match outcome {
        RecoveryOutcome::Refused { error } => &error.code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_restore_review_needs_read_only_a_listed_snapshot_and_a_sound_ledger() {
    let fake = fake_plane();
    let tokens = fake.tokens(&listed());

    let serving = fake.prepare(restore_of(&tokens[0]), serving()).await;
    assert_eq!(
        code(serving),
        ("recovery.not_read_only_local".into(), "conflict")
    );

    let corrupt = status(
        StoreState::ReadOnly,
        listed(),
        Some(DeletionLedgerStatus::Corrupt),
        None,
    );
    let refused = fake.prepare(restore_of(&tokens[0]), corrupt).await;
    assert_eq!(
        code(refused),
        ("recovery.deletion_ledger_corrupt".into(), "conflict")
    );

    // The migration snapshot rotated out since the token was read.
    let rotated = status(
        StoreState::ReadOnly,
        listed()[..1].to_vec(),
        Some(DeletionLedgerStatus::Verified { deletions: 2 }),
        None,
    );
    let gone = fake.prepare(restore_of(&tokens[1]), rotated).await;
    let error = gone.unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.category),
        ("recovery.snapshot_gone", "conflict")
    );
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));

    // A token this shell never issued, and one that is not a UUID at all:
    // the second is refused before any I/O.
    let unknown = fake
        .prepare(restore_of(&Uuid::from_u128(1).to_string()), read_only())
        .await;
    assert_eq!(code(unknown).0, "recovery.snapshot_gone");
    let malformed = prepare(fake.bridge(), &fake.plane_id, restore_of(DAILY))
        .await
        .unwrap_err();
    assert_eq!(malformed.code, "recovery.snapshot_gone");
    fake.assert_untouched().await;

    // The token of a snapshot still listed stays usable across reads.
    let view = fake
        .prepare(restore_of(&tokens[0]), read_only())
        .await
        .unwrap();
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["kind"], "restore_snapshot");
    assert_eq!(json["planeLabel"], "Build box");
    assert_eq!(json["takenAtUnixMs"], "1700000000000");
    assert_eq!(json["reason"], "daily");
    assert_eq!(json["bytes"], "4096");
    assert!(!json.to_string().contains("sqlite3"));
}

#[tokio::test]
async fn a_restore_is_sent_once_its_outcome_replays_and_the_planes_caches_are_dropped() {
    let fake = fake_plane();
    let tokens = fake.tokens(&listed());
    let view = fake
        .prepare(restore_of(&tokens[0]), read_only())
        .await
        .unwrap();
    let id = review_id(&view);
    let command_id = Uuid::parse_str(&id).unwrap();

    let (outcome, ()) = tokio::join!(fake.execute(&id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_restore(&mut reader, command_id).await;
        command_result(
            &mut writer,
            stream,
            &message,
            CommandResponse::RecoverySnapshotRestored {
                snapshot: DAILY.into(),
                replaced: "plane.sqlite3.damaged-1757000100000".into(),
            },
        )
        .await;
        // The registry's health is read again on the same connection.
        serve_status(&mut reader, &mut writer, serving()).await;
    });
    let outcome = outcome.unwrap();
    assert_eq!(
        serde_json::to_value(&outcome).unwrap(),
        json!({
            "kind": "restored",
            "takenAtUnixMs": "1700000000000",
            "reason": "daily",
            "replacedName": "plane.sqlite3.damaged-1757000100000",
        })
    );
    assert_eq!(
        fake.bridge().planes.health(fake.plane),
        crate::jet::planes::PlaneHealth::from_status(&serving())
    );
    // Snapshot tokens of the old store are gone.
    let daily = Uuid::parse_str(&tokens[0]).unwrap();
    assert_eq!(
        fake.state().name(fake.plane, IDENTITY, daily).unwrap(),
        None
    );

    // A lost IPC reply is answered from the record, without the Plane.
    let replayed = fake.execute(&id).await.unwrap();
    assert_eq!(replayed, outcome);
    fake.assert_untouched().await;
}

#[tokio::test]
async fn a_lost_reply_is_unconfirmed_and_never_resent() {
    let fake = fake_plane();
    let tokens = fake.tokens(&listed());
    let view = fake
        .prepare(restore_of(&tokens[0]), read_only())
        .await
        .unwrap();
    let id = review_id(&view);
    let command_id = Uuid::parse_str(&id).unwrap();

    // The Command reaches the Plane and the connection drops.
    let (outcome, ()) = tokio::join!(fake.execute(&id), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        expect_restore(&mut reader, command_id).await;
    });
    let outcome = outcome.unwrap();
    let RecoveryOutcome::Unconfirmed { error } = &outcome else {
        panic!("expected an unconfirmed outcome, got {outcome:?}");
    };
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));

    // Executing again replays "unconfirmed"; nothing reaches the Plane.
    assert_eq!(fake.execute(&id).await.unwrap(), outcome);
    fake.assert_untouched().await;

    // The recorded outcome resolved the review: a new one can be prepared.
    assert!(fake
        .prepare(restore_of(&tokens[0]), read_only())
        .await
        .is_ok());
}

#[tokio::test]
async fn a_daemon_refusal_is_recorded_and_another_snapshot_is_unconfirmed() {
    let fake = fake_plane();
    let tokens = fake.tokens(&listed());
    let first = review_id(
        &fake
            .prepare(restore_of(&tokens[0]), read_only())
            .await
            .unwrap(),
    );
    let (outcome, ()) = tokio::join!(fake.execute(&first), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_restore(&mut reader, Uuid::parse_str(&first).unwrap()).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::Unavailable,
            "recovery.restore_failed",
        )
        .await;
    });
    let outcome = outcome.unwrap();
    assert_eq!(refused_code(&outcome), "recovery.restore_failed");
    // Refused means the store is unchanged: its tokens stay.
    let daily = Uuid::parse_str(&tokens[0]).unwrap();
    assert!(fake
        .state()
        .name(fake.plane, IDENTITY, daily)
        .unwrap()
        .is_some());

    let second = review_id(
        &fake
            .prepare(restore_of(&tokens[0]), read_only())
            .await
            .unwrap(),
    );
    let (outcome, ()) = tokio::join!(fake.execute(&second), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) =
            expect_restore(&mut reader, Uuid::parse_str(&second).unwrap()).await;
        command_result(
            &mut writer,
            stream,
            &message,
            CommandResponse::RecoverySnapshotRestored {
                snapshot: MIGRATION.into(),
                replaced: "plane.sqlite3.damaged-1".into(),
            },
        )
        .await;
    });
    assert!(matches!(
        outcome.unwrap(),
        RecoveryOutcome::Unconfirmed { ref error } if error.code == "client.state_unavailable"
    ));
}

#[tokio::test]
async fn an_attempt_without_a_recorded_outcome_is_never_sent_again() {
    let fake = fake_plane();
    let binding = PlaneBinding {
        plane: fake.plane,
        identity: Some(IDENTITY),
    };
    let id = fake
        .state()
        .actions
        .issue(
            binding,
            RESTORE_SCOPE,
            RecoveryReview::Restore {
                snapshot: DAILY.into(),
                taken_at_unix_ms: 1,
                reason: SnapshotReason::Daily,
            },
        )
        .unwrap();
    // As if the IPC future were dropped after the send began.
    fake.state()
        .actions
        .attempt(id, fake.plane, |_| Ok(()))
        .unwrap();
    let outcome = fake.execute(&id.to_string()).await.unwrap();
    assert_eq!(refused_code(&outcome), "client.review_used");
    fake.assert_untouched().await;
    assert_eq!(fake.execute(&id.to_string()).await.unwrap(), outcome);
}

#[tokio::test]
async fn reviews_are_refused_on_another_plane_and_when_unknown() {
    let fake = fake_plane();
    let id = fake
        .state()
        .actions
        .issue(
            PlaneBinding {
                plane: fake.plane,
                identity: Some(IDENTITY),
            },
            PURGE_SCOPE,
            RecoveryReview::Purge,
        )
        .unwrap();
    let elsewhere = execute(fake.bridge(), "local", &id.to_string())
        .await
        .unwrap();
    assert_eq!(refused_code(&elsewhere), "client.review_plane_mismatch");
    let unknown = fake.execute(&Uuid::from_u128(3).to_string()).await.unwrap();
    assert_eq!(refused_code(&unknown), "recovery.review_expired");
    let malformed = fake.execute("not-a-review").await.unwrap();
    assert_eq!(refused_code(&malformed), "recovery.review_expired");
    fake.assert_untouched().await;
    let invalid = execute(fake.bridge(), "../jetd.sock", &id.to_string())
        .await
        .unwrap_err();
    assert_eq!(invalid.category, "invalid_input");
}

#[tokio::test]
async fn a_send_that_cannot_connect_is_refused_because_nothing_was_sent() {
    let directory = tempfile::tempdir().unwrap();
    let bridge = crate::jet::JetBridge::for_test(
        directory.path(),
        CLIENT,
        std::sync::Arc::new(crate::jet::planes::spawner::fake::FakeSpawner::default()),
        std::sync::Arc::new(crate::jet::keystore::IdentityKeys::new(
            std::sync::Arc::new(crate::jet::keystore::tests::CountingStore::new(
                crate::jet::keystore::tests::Fail::Nothing,
            )),
        )),
    );
    let id = bridge
        .system
        .recovery
        .actions
        .issue(
            PlaneBinding {
                plane: PlaneId::Local,
                identity: None,
            },
            PURGE_SCOPE,
            RecoveryReview::Purge,
        )
        .unwrap();
    let outcome = execute(&bridge, "local", &id.to_string()).await.unwrap();
    let RecoveryOutcome::Refused { error } = &outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert_eq!(error.category, "offline");
    assert_eq!(error.plane_id.as_deref(), Some("local"));
    // Recorded: the same review is answered without another attempt.
    assert_eq!(
        execute(&bridge, "local", &id.to_string()).await.unwrap(),
        outcome
    );
}

// ---------------------------------------------------------------------------
// Purge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_purge_needs_a_serving_store_a_verified_ledger_and_a_trusted_audit() {
    let fake = fake_plane();
    let degraded = SecurityState::Degraded {
        breach: AuditBreach::HeadNotInStore,
        epoch: 2,
        head: None,
        store_sequence: 9,
    };
    let unavailable = [
        status(
            StoreState::Serving,
            listed(),
            Some(DeletionLedgerStatus::Verified { deletions: 2 }),
            Some(degraded),
        ),
        status(
            StoreState::Serving,
            listed(),
            Some(DeletionLedgerStatus::Verified { deletions: 2 }),
            None,
        ),
        status(
            StoreState::Serving,
            listed(),
            Some(DeletionLedgerStatus::Corrupt),
            Some(SecurityState::Trusted),
        ),
        status(
            StoreState::Serving,
            listed(),
            None,
            Some(SecurityState::Trusted),
        ),
        read_only(),
    ];
    for plane_status in unavailable {
        let refused = fake
            .prepare(RecoveryAction::PurgeSnapshots {}, plane_status)
            .await;
        assert_eq!(
            code(refused),
            ("recovery.purge_unavailable".into(), "conflict")
        );
    }

    let view = fake
        .prepare(RecoveryAction::PurgeSnapshots {}, serving())
        .await
        .unwrap();
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(
        json,
        json!({
            "kind": "purge_snapshots",
            "reviewId": review_id(&view),
            "planeLabel": "Build box",
            "snapshotCount": 2,
            "totalBytes": "8192",
            "deletionsRecorded": "2",
            "includesRollback": true,
        })
    );
    // A second review while the first is unsent is allowed; only an
    // unresolved send blocks the scope.
    assert!(fake
        .prepare(RecoveryAction::PurgeSnapshots {}, serving())
        .await
        .is_ok());
}

#[tokio::test]
async fn a_purge_reports_what_it_removed_and_a_refusal_is_definite() {
    let fake = fake_plane();
    let tokens = fake.tokens(&listed());
    let id = review_id(
        &fake
            .prepare(RecoveryAction::PurgeSnapshots {}, serving())
            .await
            .unwrap(),
    );
    let (outcome, ()) = tokio::join!(fake.execute(&id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_purge(&mut reader, Uuid::parse_str(&id).unwrap()).await;
        command_result(
            &mut writer,
            stream,
            &message,
            CommandResponse::RecoverySnapshotsPurged {
                snapshot: "plane-1757100000000-maintenance.sqlite3".into(),
                removed: vec![DAILY.into(), MIGRATION.into()],
            },
        )
        .await;
    });
    assert_eq!(
        serde_json::to_value(outcome.unwrap()).unwrap(),
        json!({"kind": "purged", "removedCount": 2})
    );
    let daily = Uuid::parse_str(&tokens[0]).unwrap();
    assert_eq!(
        fake.state().name(fake.plane, IDENTITY, daily).unwrap(),
        None
    );

    let id = review_id(
        &fake
            .prepare(RecoveryAction::PurgeSnapshots {}, serving())
            .await
            .unwrap(),
    );
    let (outcome, ()) = tokio::join!(fake.execute(&id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_purge(&mut reader, Uuid::parse_str(&id).unwrap()).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::Conflict,
            "security.audit_degraded",
        )
        .await;
    });
    assert_eq!(refused_code(&outcome.unwrap()), "security.audit_degraded");
}

#[tokio::test]
async fn a_restore_stands_when_the_status_re_read_fails_and_hides_a_path() {
    let fake = fake_plane();
    let tokens = fake.tokens(&listed());
    let view = fake
        .prepare(restore_of(&tokens[0]), read_only())
        .await
        .unwrap();
    let id = review_id(&view);
    let (outcome, ()) = tokio::join!(fake.execute(&id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_restore(&mut reader, Uuid::parse_str(&id).unwrap()).await;
        command_result(
            &mut writer,
            stream,
            &message,
            CommandResponse::RecoverySnapshotRestored {
                snapshot: DAILY.into(),
                replaced: "/abs/path".into(),
            },
        )
        .await;
        // The follow-up status read fails; the restore still stands.
    });
    let outcome = outcome.unwrap();
    assert!(matches!(
        outcome,
        RecoveryOutcome::Restored {
            replaced_name: None,
            ..
        }
    ));
}
