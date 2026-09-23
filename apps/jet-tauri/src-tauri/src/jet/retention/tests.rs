use std::time::Duration;

use jet_protocol::{
    ClientMessage, CommandRequest, CommandResponse, Conversation, ConversationSnapshot,
    ConversationTrash, ErrorCategory, Name, NameSource, QueryRequest, QueryResponse,
    ResolvedSetting, RetentionPolicy, RetentionPreview, RetentionProtection, ServerMessage,
    SettingKey, SettingScope, SettingSelection, SettingSnapshot, SettingSource, SettingValue,
    StreamId, TrashEntry, TrashReason, WireError,
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

const CLIENT: Uuid = Uuid::from_u128(0x7a54);
const TASK: Uuid = Uuid::from_u128(0x7a5c);
const TASK_ID: &str = "00000000-0000-0000-0000-000000007a5c";

// ---------------------------------------------------------------------------
// Pure views and the attempt check
// ---------------------------------------------------------------------------

fn entry(reason: TrashReason, trashed_at: i64) -> TrashEntry {
    TrashEntry {
        conversation_id: TASK,
        reason,
        trashed_at_unix_ms: trashed_at,
        expires_at_unix_ms: trashed_at + 30 * 86_400_000,
    }
}

#[test]
fn every_reason_maps_and_only_a_transfer_tombstone_is_not_restorable() {
    let reasons = [
        (TrashReason::ManualForget, "manual_forget", true),
        (TrashReason::AutomaticForget, "automatic_forget", true),
        (TrashReason::DeleteEverywhere, "delete_everywhere", true),
        (TrashReason::AutodeleteRule, "autodelete_rule", true),
        (
            TrashReason::AutodeleteEverywhere,
            "autodelete_everywhere",
            true,
        ),
        (TrashReason::PlaneTransfer, "plane_transfer", false),
    ];
    for (reason, name, restorable) in reasons {
        let view = entry_view(&entry(reason, 5));
        assert_eq!(view.reason, name);
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(name));
        assert_eq!(view.restorable, restorable, "{name}");
    }
    assert_eq!(
        serde_json::to_value(entry_view(&entry(TrashReason::ManualForget, 5))).unwrap(),
        json!({
            "conversationId": TASK.to_string(),
            "reason": "manual_forget",
            "trashedAtUnixMs": "5",
            "expiresAtUnixMs": (5 + 30 * 86_400_000_i64).to_string(),
            "restorable": true,
        })
    );
}

#[test]
fn a_full_list_is_capped_and_a_longer_one_is_refused() {
    let list = |count: usize| ConversationTrash {
        cursor: 9,
        entries: (0..count)
            .map(|index| TrashEntry {
                conversation_id: Uuid::from_u128(index as u128 + 1),
                ..entry(TrashReason::ManualForget, 1)
            })
            .collect(),
    };
    let view = trash_view(PlaneId::Local, "This computer".into(), &list(255), Some(30)).unwrap();
    assert!(!view.capped);
    let view = trash_view(PlaneId::Local, "This computer".into(), &list(256), None).unwrap();
    assert!(view.capped);
    assert_eq!(view.entries.len(), 256);
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["planeId"], "local");
    assert_eq!(json["cursor"], "9");
    assert_eq!(json["graceDays"], serde_json::Value::Null);
    let error = trash_view(PlaneId::Local, "This computer".into(), &list(257), None).unwrap_err();
    assert_eq!(error.code, "client.state_unavailable");
}

fn review(active_run: bool, unchecked: bool) -> TrashReview {
    TrashReview {
        conversation: TASK,
        recorded_active_run: active_run,
        recorded_pending_turn: false,
        workspace_unchecked: unchecked,
        mode: None,
    }
}

#[test]
fn the_first_attempt_locks_the_mode_and_stopping_work_needs_an_acknowledgement() {
    let mut idle = review(false, false);
    lock_mode(&mut idle, TrashMode::DeleteEverywhere, false).unwrap();
    assert_eq!(idle.mode, Some(TrashMode::DeleteEverywhere));
    let locked = lock_mode(&mut idle, TrashMode::Forget, false).unwrap_err();
    assert_eq!(
        (locked.code.as_str(), locked.category),
        ("retention.mode_locked", "conflict")
    );

    for mut busy in [review(true, false), review(false, true)] {
        let refused = lock_mode(&mut busy, TrashMode::DeleteEverywhere, false).unwrap_err();
        assert_eq!(refused.code, "retention.stop_unacknowledged");
        // A refused check locks nothing.
        assert_eq!(busy.mode, None);
        // Forget stops nothing, so it needs no acknowledgement.
        lock_mode(&mut busy.clone(), TrashMode::Forget, false).unwrap();
        lock_mode(&mut busy, TrashMode::DeleteEverywhere, true).unwrap();
    }
}

#[test]
fn a_restored_store_drops_that_planes_trash_index_restores_and_reviews() {
    let state = RetentionState::default();
    let remote = PlaneId::Remote(Uuid::from_u128(0x7a5f));
    let binding = |plane| PlaneBinding {
        plane,
        identity: None,
    };
    for plane in [PlaneId::Local, remote] {
        state.remember(plane, TASK, Some(40)).unwrap();
        state.restore_id((plane, TASK, 40)).unwrap();
    }
    let local_review = state
        .trash_reviews
        .issue(binding(PlaneId::Local), TASK, review(false, false))
        .unwrap();
    // Even a review whose outcome is unknown: it was about the old store.
    state
        .trash_reviews
        .attempt(local_review, PlaneId::Local, |_| Ok(()))
        .unwrap();
    let remote_review = state
        .trash_reviews
        .issue(binding(remote), TASK, review(false, false))
        .unwrap();

    state.plane_restored(PlaneId::Local);

    assert_eq!(state.trashed_at(PlaneId::Local, TASK).unwrap(), None);
    assert_eq!(state.trashed_at(remote, TASK).unwrap(), Some(40));
    let restores = state.restores.lock().unwrap();
    assert_eq!(
        restores.keys().copied().collect::<Vec<_>>(),
        vec![(remote, TASK, 40)]
    );
    drop(restores);
    assert_eq!(
        state
            .trash_reviews
            .attempt(local_review, PlaneId::Local, |_| Ok(()))
            .unwrap_err()
            .code,
        "retention.review_expired"
    );
    assert!(state
        .trash_reviews
        .attempt(remote_review, remote, |_| Ok(()))
        .is_ok());
    // A new review of the same task can be issued at once.
    assert!(state
        .trash_reviews
        .issue(binding(PlaneId::Local), TASK, review(false, false))
        .is_ok());
}

#[test]
fn names_are_bounded_distinct_uuids() {
    let id = |n: u128| Uuid::from_u128(n).to_string();
    assert!(parse_names(&[id(1), id(2)]).is_ok());
    let many: Vec<String> = (1..=32).map(id).collect();
    assert_eq!(parse_names(&many).unwrap().len(), 32);
    let too_many: Vec<String> = (1..=33).map(id).collect();
    for invalid in [vec![], too_many, vec![id(1), id(1)], vec!["../x".into()]] {
        assert_eq!(
            parse_names(&invalid).unwrap_err().code,
            "retention.names_invalid"
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
    let socket = setup.directory.path().join("trash-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0x7a5e);
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

    fn binding(&self) -> PlaneBinding {
        PlaneBinding {
            plane: self.plane,
            identity: None,
        }
    }

    /// Issues a review directly, as a preview with these findings would.
    fn issue(&self, active_run: bool, unchecked: bool) -> String {
        self.bridge()
            .retention
            .trash_reviews
            .issue(self.binding(), TASK, review(active_run, unchecked))
            .unwrap()
            .to_string()
    }

    async fn trash(
        &self,
        review_id: &str,
        mode: TrashMode,
        acknowledge: bool,
    ) -> Result<TrashOutcome, PublicError> {
        trash_conversation_for(self.bridge(), &self.plane_id, review_id, mode, acknowledge).await
    }

    async fn restore(&self) -> Result<RestoreOutcome, PublicError> {
        restore_conversation_for(self.bridge(), &self.plane_id, &TASK.to_string()).await
    }

    /// No connection reached the Plane.
    async fn assert_untouched(&self) {
        let pending = tokio::time::timeout(Duration::from_millis(20), self.listener.accept()).await;
        assert!(pending.is_err(), "the Plane was contacted");
    }
}

async fn answer(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    result: QueryResponse,
) {
    reply(
        writer,
        stream,
        ServerMessage::QueryResult {
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

fn preview(protections: Vec<RetentionProtection>, trash: Option<TrashEntry>) -> RetentionPreview {
    RetentionPreview {
        conversation_id: TASK,
        protections,
        trash,
        audit_records: 3,
    }
}

async fn expect_preview(reader: &mut Reader) -> (StreamId, ClientMessage) {
    let (stream, message) = next_message(reader).await;
    assert!(
        matches!(
            message,
            ClientMessage::Query {
                query: QueryRequest::RetentionPreview { conversation_id },
                ..
            } if conversation_id == TASK
        ),
        "unexpected request {message:?}"
    );
    (stream, message)
}

/// Serves the reads of `preview_trash` after the retention preview: the
/// Conversation for its title and the grace-days setting.
async fn serve_title_and_grace(reader: &mut Reader, writer: &mut Writer) {
    let (stream, message) = next_message(reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::Conversation { conversation_id },
            ..
        } if conversation_id == TASK
    ));
    answer(
        writer,
        stream,
        &message,
        QueryResponse::Conversation(Box::new(snapshot(TASK, "Fix login"))),
    )
    .await;
    let (stream, message) = next_message(reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::Settings {
                scope: SettingScope::Plane,
                selection: SettingSelection::Key {
                    key: SettingKey::RetentionTrashGraceDays
                },
            },
            ..
        }
    ));
    answer(
        writer,
        stream,
        &message,
        QueryResponse::Settings(SettingSnapshot {
            cursor: 4,
            scope: SettingScope::Plane,
            settings: vec![ResolvedSetting {
                key: SettingKey::RetentionTrashGraceDays,
                value: SettingValue::Count(30),
                source: SettingSource::BuiltIn,
            }],
        }),
    )
    .await;
}

fn snapshot(id: Uuid, title: &str) -> ConversationSnapshot {
    ConversationSnapshot {
        cursor: 4,
        conversation: Conversation {
            conversation_id: id,
            revision: None,
            retention: RetentionPolicy::Retain,
            working_tree: None,
            origin: None,
            name: Some(Name {
                value: title.into(),
                source: NameSource::Manual,
            }),
            created_at_unix_ms: 1,
        },
        workspace: None,
        runs: Vec::new(),
    }
}

/// The next request must be the reviewed trash Command under `command_id`.
async fn expect_trash(
    reader: &mut Reader,
    command_id: Uuid,
    mode: TrashMode,
) -> (StreamId, ClientMessage) {
    let (stream, message) = next_message(reader).await;
    match (&message, mode) {
        (
            ClientMessage::Command {
                command_id: sent,
                command: CommandRequest::ForgetConversation { conversation_id },
                ..
            },
            TrashMode::Forget,
        )
        | (
            ClientMessage::Command {
                command_id: sent,
                command: CommandRequest::DeleteConversationEverywhere { conversation_id },
                ..
            },
            TrashMode::DeleteEverywhere,
        ) if *sent == command_id && *conversation_id == TASK => {}
        (other, _) => panic!("unexpected request {other:?}"),
    }
    (stream, message)
}

async fn trashed(writer: &mut Writer, stream: StreamId, message: &ClientMessage, trashed_at: i64) {
    reply(
        writer,
        stream,
        ServerMessage::CommandResult {
            id: request_id(message),
            result: CommandResponse::ConversationTrashed {
                entry: entry(TrashReason::ManualForget, trashed_at),
            },
        },
    )
    .await;
}

async fn assert_closed(reader: &mut Reader) {
    assert!(reader.read().await.is_err(), "no further request expected");
}

// ---------------------------------------------------------------------------
// Reviews
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unreadable_workspace_still_issues_a_review_that_needs_the_acknowledgement() {
    let fake = fake_plane();
    let (view, ()) = tokio::join!(
        preview_trash_for(fake.bridge(), &fake.plane_id, TASK_ID),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            refuse(
                &mut writer,
                stream,
                &message,
                ErrorCategory::Unavailable,
                WORKSPACE_UNREADABLE,
            )
            .await;
            serve_title_and_grace(&mut reader, &mut writer).await;
            // The preview sent no Command.
            assert_closed(&mut reader).await;
        }
    );
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(view["workspaceUnchecked"], true);
    assert_eq!(view["protections"], serde_json::Value::Null);
    assert_eq!(view["auditRecords"], serde_json::Value::Null);
    assert_eq!(view["stopAcknowledgementRequired"], true);
    assert_eq!(view["title"], "Fix login");
    assert_eq!(view["planeLabel"], "Build box");
    assert_eq!(view["graceDays"], 30);
    let review_id = view["reviewId"].as_str().unwrap().to_owned();
    assert!(Uuid::parse_str(&review_id).is_ok());

    let refused = fake
        .trash(&review_id, TrashMode::DeleteEverywhere, false)
        .await
        .unwrap();
    let TrashOutcome::Refused { error } = refused else {
        panic!("expected a refusal");
    };
    assert_eq!(error.code, "retention.stop_unacknowledged");
    fake.assert_untouched().await;
}

#[tokio::test]
async fn a_preview_lists_protections_and_an_already_trashed_task_gets_no_review() {
    let fake = fake_plane();
    let (view, ()) = tokio::join!(
        preview_trash_for(fake.bridge(), &fake.plane_id, TASK_ID),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            let body = preview(
                vec![
                    RetentionProtection::ActiveRun,
                    RetentionProtection::UnpushedWork,
                ],
                None,
            );
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::RetentionPreview(body),
            )
            .await;
            serve_title_and_grace(&mut reader, &mut writer).await;
        }
    );
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(
        view["protections"],
        json!([
            {"kind": "active_run", "label": "Activity in progress"},
            {"kind": "unpushed_work", "label": "Unpushed commits"},
        ])
    );
    assert_eq!(view["activeRun"], true);
    assert_eq!(view["stopAcknowledgementRequired"], true);
    assert_eq!(view["auditRecords"], "3");

    let (view, ()) = tokio::join!(
        preview_trash_for(fake.bridge(), &fake.plane_id, TASK_ID),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            let body = preview(vec![], Some(entry(TrashReason::ManualForget, 50)));
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::RetentionPreview(body),
            )
            .await;
            serve_title_and_grace(&mut reader, &mut writer).await;
        }
    );
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(view["reviewId"], "");
    assert_eq!(view["trash"]["trashedAtUnixMs"], "50");
    // The entry the shell read keys a later restore.
    assert_eq!(
        fake.bridge()
            .retention
            .trashed_at(fake.plane, TASK)
            .unwrap(),
        Some(50)
    );
}

#[tokio::test]
async fn an_uncertain_forget_locks_its_mode_blocks_new_reviews_and_resends_one_command() {
    let fake = fake_plane();
    let review_id = fake.issue(false, false);
    let command_id = Uuid::parse_str(&review_id).unwrap();

    // The Command reaches the Plane but the reply is lost.
    let (first, ()) = tokio::join!(fake.trash(&review_id, TrashMode::Forget, false), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        expect_trash(&mut reader, command_id, TrashMode::Forget).await;
    });
    assert_eq!(first.unwrap_err().category, "offline");

    // The other mode is locked out, with nothing sent.
    let locked = fake
        .trash(&review_id, TrashMode::DeleteEverywhere, true)
        .await
        .unwrap();
    assert!(
        matches!(locked, TrashOutcome::Refused { ref error } if error.code == "retention.mode_locked")
    );
    fake.assert_untouched().await;

    // A second review of the same task waits for the first to resolve.
    let (second, ()) = tokio::join!(
        preview_trash_for(fake.bridge(), &fake.plane_id, TASK_ID),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::RetentionPreview(preview(vec![], None)),
            )
            .await;
            serve_title_and_grace(&mut reader, &mut writer).await;
        }
    );
    let error = second.unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.category),
        ("retention.request_unresolved", "conflict")
    );

    // Try again resends the same Command ID, with no re-read.
    let (retried, ()) = tokio::join!(fake.trash(&review_id, TrashMode::Forget, false), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_trash(&mut reader, command_id, TrashMode::Forget).await;
        trashed(&mut writer, stream, &message, 70).await;
    });
    let TrashOutcome::Trashed { entry } = retried.unwrap() else {
        panic!("expected the task in Jet Trash");
    };
    assert_eq!(entry.trashed_at_unix_ms, "70");
    assert_eq!(
        fake.bridge()
            .retention
            .trashed_at(fake.plane, TASK)
            .unwrap(),
        Some(70)
    );

    // A lost IPC reply replays the recorded outcome without contacting the Plane.
    let replayed = fake
        .trash(&review_id, TrashMode::Forget, false)
        .await
        .unwrap();
    assert!(matches!(replayed, TrashOutcome::Trashed { .. }));
    fake.assert_untouched().await;
}

#[tokio::test]
async fn a_retry_whose_plane_moved_stays_uncertain() {
    let fake = fake_plane();
    let review_id = fake.issue(false, false);
    let command_id = Uuid::parse_str(&review_id).unwrap();
    let (first, ()) = tokio::join!(fake.trash(&review_id, TrashMode::Forget, false), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        expect_trash(&mut reader, command_id, TrashMode::Forget).await;
    });
    assert_eq!(first.unwrap_err().category, "offline");

    // The Plane entry is gone: the first send may still have been applied,
    // so the retry is not recorded as a refusal.
    let PlaneId::Remote(id) = fake.plane else {
        unreachable!()
    };
    fake.bridge().planes.remove_for_test(id);
    let moved = fake
        .trash(&review_id, TrashMode::Forget, false)
        .await
        .unwrap_err();
    assert_eq!(moved.code, "plane.review_moved");
    assert_eq!(moved.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    fake.assert_untouched().await;

    // Once the Plane is back, Try again still resends the same Command ID.
    fake.bridge().planes.insert_for_test(
        id,
        "Build box",
        PlaneClient::new(
            fake.setup.directory.path().join("trash-jetd.sock"),
            CLIENT,
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        ),
    );
    let (retried, ()) = tokio::join!(fake.trash(&review_id, TrashMode::Forget, false), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_trash(&mut reader, command_id, TrashMode::Forget).await;
        trashed(&mut writer, stream, &message, 71).await;
    });
    assert!(matches!(retried.unwrap(), TrashOutcome::Trashed { .. }));
}

#[tokio::test]
async fn activity_started_after_the_review_makes_delete_everywhere_stale_without_a_command() {
    let fake = fake_plane();
    let review_id = fake.issue(false, false);
    let (outcome, ()) = tokio::join!(
        fake.trash(&review_id, TrashMode::DeleteEverywhere, false),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            let body = preview(vec![RetentionProtection::ActiveRun], None);
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::RetentionPreview(body),
            )
            .await;
            assert_closed(&mut reader).await;
        }
    );
    let TrashOutcome::Refused { error } = outcome.unwrap() else {
        panic!("expected a stale review");
    };
    assert_eq!(
        (error.code.as_str(), error.category),
        ("retention.review_stale", "conflict")
    );
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    // The outcome is recorded: asking again does not reach the Plane.
    let again = fake
        .trash(&review_id, TrashMode::DeleteEverywhere, true)
        .await
        .unwrap();
    assert!(
        matches!(again, TrashOutcome::Refused { ref error } if error.code == "retention.review_stale")
    );
    fake.assert_untouched().await;
}

#[tokio::test]
async fn delete_everywhere_sends_after_a_re_read_that_shows_only_reviewed_activity() {
    let fake = fake_plane();
    let review_id = fake.issue(true, false);
    let command_id = Uuid::parse_str(&review_id).unwrap();
    let (outcome, ()) = tokio::join!(
        fake.trash(&review_id, TrashMode::DeleteEverywhere, true),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            let body = preview(vec![RetentionProtection::ActiveRun], None);
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::RetentionPreview(body),
            )
            .await;
            let (stream, message) =
                expect_trash(&mut reader, command_id, TrashMode::DeleteEverywhere).await;
            trashed(&mut writer, stream, &message, 90).await;
        }
    );
    assert!(matches!(outcome.unwrap(), TrashOutcome::Trashed { .. }));
}

#[tokio::test]
async fn a_daemon_refusal_is_recorded_and_ends_the_review() {
    let fake = fake_plane();
    let review_id = fake.issue(true, false);
    let command_id = Uuid::parse_str(&review_id).unwrap();
    let (outcome, ()) = tokio::join!(fake.trash(&review_id, TrashMode::Forget, false), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = expect_trash(&mut reader, command_id, TrashMode::Forget).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::Conflict,
            "retention.live_work",
        )
        .await;
    });
    let TrashOutcome::Refused { error } = outcome.unwrap() else {
        panic!("expected a refusal");
    };
    assert_eq!(error.code, "retention.live_work");
    // Resolved: a new review of the task can be issued.
    fake.issue(false, false);
}

#[tokio::test]
async fn a_review_from_one_plane_is_refused_on_another() {
    let fake = fake_plane();
    let review_id = fake.issue(false, false);
    let outcome =
        trash_conversation_for(fake.bridge(), "local", &review_id, TrashMode::Forget, false)
            .await
            .unwrap();
    let TrashOutcome::Refused { error } = outcome else {
        panic!("expected a refusal");
    };
    assert_eq!(error.code, "client.review_plane_mismatch");
    assert_eq!(error.plane_id.as_deref(), Some("local"));
    fake.assert_untouched().await;
    // Still unattempted on its own Plane: a new review is not blocked.
    fake.issue(false, false);
}

// ---------------------------------------------------------------------------
// Status, restore and names
// ---------------------------------------------------------------------------

async fn status(fake: &Fake, trash: Option<TrashEntry>) -> serde_json::Value {
    let (view, ()) = tokio::join!(
        load_trash_status_for(fake.bridge(), &fake.plane_id, TASK_ID),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_preview(&mut reader).await;
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::RetentionPreview(preview(vec![], trash)),
            )
            .await;
        }
    );
    serde_json::to_value(view.unwrap()).unwrap()
}

async fn expect_restore(reader: &mut Reader) -> (StreamId, ClientMessage, Uuid) {
    let (stream, message) = next_message(reader).await;
    let ClientMessage::Command {
        command_id,
        command: CommandRequest::RestoreConversation { conversation_id },
        ..
    } = &message
    else {
        panic!("unexpected request {message:?}");
    };
    assert_eq!(*conversation_id, TASK);
    let command_id = *command_id;
    (stream, message, command_id)
}

#[tokio::test]
async fn restoring_a_task_trashed_again_uses_a_new_command_id() {
    let fake = fake_plane();
    let unknown = fake.restore().await.unwrap();
    assert!(
        matches!(unknown, RestoreOutcome::Refused { ref error } if error.code == "retention.trash_unknown")
    );
    fake.assert_untouched().await;

    let view = status(&fake, Some(entry(TrashReason::ManualForget, 100))).await;
    assert_eq!(view["trash"]["trashedAtUnixMs"], "100");

    // The fake daemon applies the restore but the reply is lost.
    let (first, first_id) = tokio::join!(fake.restore(), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        expect_restore(&mut reader).await.2
    });
    assert_eq!(first.unwrap_err().category, "offline");

    // Try again keeps the Command ID of that staging.
    let (again, again_id) = tokio::join!(fake.restore(), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        expect_restore(&mut reader).await.2
    });
    assert!(again.is_err());
    assert_eq!(again_id, first_id);

    // The task was trashed again meanwhile: a new staging, a new ID.
    status(&fake, Some(entry(TrashReason::ManualForget, 200))).await;
    let (restored, second_id) = tokio::join!(fake.restore(), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message, id) = expect_restore(&mut reader).await;
        reply(
            &mut writer,
            stream,
            ServerMessage::CommandResult {
                id: request_id(&message),
                result: CommandResponse::ConversationRestored {
                    conversation_id: TASK,
                },
            },
        )
        .await;
        id
    });
    assert_ne!(second_id, first_id);
    assert_eq!(
        restored.unwrap(),
        RestoreOutcome::Restored {
            conversation_id: TASK.to_string()
        }
    );
    // Restored: no longer indexed, so a stale Restore asks for a reload.
    assert_eq!(
        fake.bridge()
            .retention
            .trashed_at(fake.plane, TASK)
            .unwrap(),
        None
    );
    assert!(fake.bridge().retention.restores.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_definite_restore_refusal_releases_its_command_id() {
    let fake = fake_plane();
    status(&fake, Some(entry(TrashReason::ManualForget, 100))).await;
    let (outcome, ()) = tokio::join!(fake.restore(), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message, _) = expect_restore(&mut reader).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::Conflict,
            "retention.not_trashed",
        )
        .await;
    });
    let RestoreOutcome::Refused { error } = outcome.unwrap() else {
        panic!("expected a refusal");
    };
    assert_eq!(error.code, "retention.not_trashed");
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    assert!(fake.bridge().retention.restores.lock().unwrap().is_empty());
    assert_eq!(
        fake.bridge()
            .retention
            .trashed_at(fake.plane, TASK)
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_trash_list_replaces_the_index_and_names_resolve_on_one_connection() {
    let fake = fake_plane();
    let other = Uuid::from_u128(0x0de);
    let (view, ()) = tokio::join!(load_trash_for(fake.bridge(), &fake.plane_id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = next_message(&mut reader).await;
        assert!(matches!(
            message,
            ClientMessage::Query {
                query: QueryRequest::ConversationTrash,
                ..
            }
        ));
        let list = ConversationTrash {
            cursor: 12,
            entries: vec![
                entry(TrashReason::PlaneTransfer, 10),
                TrashEntry {
                    conversation_id: other,
                    ..entry(TrashReason::ManualForget, 20)
                },
            ],
        };
        answer(
            &mut writer,
            stream,
            &message,
            QueryResponse::ConversationTrash(list),
        )
        .await;
        // The grace-days read fails: the list still loads, without the number.
        let (stream, message) = next_message(&mut reader).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::Unauthorized,
            "unauthorized",
        )
        .await;
    });
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(view["planeLabel"], "Build box");
    assert_eq!(view["graceDays"], serde_json::Value::Null);
    assert_eq!(view["capped"], false);
    assert_eq!(view["entries"][0]["restorable"], false);
    assert_eq!(
        fake.bridge()
            .retention
            .trashed_at(fake.plane, other)
            .unwrap(),
        Some(20)
    );

    let ids = vec![TASK.to_string(), other.to_string()];
    let (names, ()) = tokio::join!(
        resolve_names_for(fake.bridge(), &fake.plane_id, &ids),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::Conversation(Box::new(snapshot(TASK, "Fix login"))),
            )
            .await;
            let (stream, message) = next_message(&mut reader).await;
            refuse(
                &mut writer,
                stream,
                &message,
                ErrorCategory::NotFound,
                "conversation.not_found",
            )
            .await;
            assert_closed(&mut reader).await;
        }
    );
    assert_eq!(
        serde_json::to_value(names.unwrap()).unwrap(),
        json!([
            {"conversationId": TASK.to_string(), "title": "Fix login"},
            {"conversationId": other.to_string(), "title": null},
        ])
    );
    // Reading a name never becomes the remembered selection.
    assert_eq!(
        fake.bridge().conversations.restored_selection().unwrap(),
        None
    );
}
