use std::time::{Duration, Instant};

use jet_protocol::{
    Actor, AuditBreach, ClientMessage, CommandRequest, CommandResponse, ErrorCategory, Event,
    PlaneStatus, QueryRequest, QueryResponse, RecoveryState, RecoveryStatus, ResolvedSetting,
    SecurityState, ServerMessage, SettingKey, SettingScope, SettingSelection, SettingSnapshot,
    SettingSource, SettingValue, StreamId, WireError,
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
        unit_tests::{accept, next_message, reply, request_id, status},
        EventSummary, NativeUpdate, PlaneClient,
    },
    enrollment::tests::{setup, Setup},
    planes::{PlaneBinding, PlaneId},
};

type Reader = jet_protocol::FrameReader<OwnedReadHalf>;
type Writer = jet_protocol::FrameWriter<OwnedWriteHalf>;

const CLIENT: Uuid = Uuid::from_u128(0x5e77);
const PROJECT: Uuid = Uuid::from_u128(0x9);

fn local() -> PlaneBinding {
    PlaneBinding {
        plane: PlaneId::Local,
        identity: None,
    }
}

fn remote(n: u128) -> PlaneBinding {
    PlaneBinding {
        plane: PlaneId::Remote(Uuid::from_u128(n)),
        identity: None,
    }
}

fn resolved(key: SettingKey, value: SettingValue, source: SettingSource) -> ResolvedSetting {
    ResolvedSetting { key, value, source }
}

fn built_in(key: SettingKey, value: SettingValue) -> ResolvedSetting {
    resolved(key, value, SettingSource::BuiltIn)
}

fn plane_set(key: SettingKey, value: SettingValue) -> ResolvedSetting {
    resolved(
        key,
        value,
        SettingSource::Scope {
            scope: SettingScope::Plane,
        },
    )
}

/// The documented table of wave 3.2 §3.2, written as an exhaustive match
/// so a new key fails to build until it is placed.
fn documented_scopes(key: SettingKey) -> &'static [ScopeKind] {
    use ScopeKind::{Conversation, Plane, Project};
    match key {
        SettingKey::StorageDisposableMiB
        | SettingKey::GitMessageInstructions
        | SettingKey::RetentionTrashGraceDays
        | SettingKey::SecurityAuditRetentionDays
        | SettingKey::UtilityGitText
        | SettingKey::UtilityContentConsent
        | SettingKey::UtilityAccountBinding
        | SettingKey::UtilityAutodeleteCompilation
        | SettingKey::DeveloperMode
        | SettingKey::AutomaticReview
        | SettingKey::AutomaticReviewBinding
        | SettingKey::AutomaticReviewConsent
        | SettingKey::ArtifactMaxMiB
        | SettingKey::ArtifactRunMiB
        | SettingKey::EnergyConcurrency
        | SettingKey::EnergyLowPowerConcurrency
        | SettingKey::EnergyConstrained
        | SettingKey::EnergyForegroundOverride => &[Plane],
        SettingKey::GitAutoCommit
        | SettingKey::GitAutoBranch
        | SettingKey::GitAutoPush
        | SettingKey::GitAutoDraftPullRequest
        | SettingKey::GitBranchPrefix => &[Project, Conversation],
        SettingKey::UtilityAutomaticNaming => &[Plane, Project, Conversation],
    }
}

// ---------------------------------------------------------------------------
// Keys, scopes and values
// ---------------------------------------------------------------------------

#[test]
fn every_key_is_placed_once_as_documented_and_spelled_as_the_protocol() {
    let mut spellings = std::collections::BTreeSet::new();
    for (key, scopes) in SETTING_SCOPES {
        assert_eq!(scopes, documented_scopes(key), "{}", key_spelling(key));
        assert_eq!(serde_json::to_value(key).unwrap(), json!(key_spelling(key)));
        assert_eq!(parse_key(key_spelling(key)).unwrap(), key);
        assert!(spellings.insert(key_spelling(key)), "duplicate key");
    }
    assert_eq!(spellings.len(), 24);
    let long = "a".repeat(65);
    for invalid in ["", "git.auto", "<script>", "GIT.AUTO_PUSH", long.as_str()] {
        assert_eq!(parse_key(invalid).unwrap_err().code, "settings.key_invalid");
    }
}

#[test]
fn scope_table_allows_only_documented_scopes() {
    let project = SettingScope::Project {
        project_id: PROJECT,
    };
    assert!(!allowed(SettingKey::GitAutoPush, &SettingScope::Plane));
    assert!(allowed(SettingKey::GitAutoPush, &project));
    assert!(!allowed(SettingKey::AutomaticReview, &project));
    assert!(allowed(SettingKey::AutomaticReview, &SettingScope::Plane));
    assert!(allowed(
        SettingKey::UtilityAutomaticNaming,
        &SettingScope::Plane
    ));
    assert!(allowed(SettingKey::UtilityAutomaticNaming, &project));
}

#[test]
fn values_are_checked_for_shape_and_size_only() {
    let text = |value: &str| SettingValue::Text(value.into());
    let ok = |key, current: &SettingValue, value: &SettingValue| {
        validate_value(key, current, value).is_ok()
    };
    let flag = SettingValue::Flag(false);
    let count = SettingValue::Count(3);
    // The variant must match the snapshot's.
    assert!(ok(
        SettingKey::EnergyConstrained,
        &flag,
        &SettingValue::Flag(true)
    ));
    assert!(!ok(SettingKey::EnergyConstrained, &flag, &count));
    assert!(!ok(SettingKey::EnergyConcurrency, &count, &text("3")));
    // Floors are the daemon's: zero passes the shell.
    assert!(ok(
        SettingKey::RetentionTrashGraceDays,
        &count,
        &SettingValue::Count(0)
    ));
    // Text: empty allowed, 2,048 bytes allowed, one more refused.
    let prefix = text("jet/");
    assert!(ok(SettingKey::GitBranchPrefix, &prefix, &text("")));
    assert!(ok(
        SettingKey::GitBranchPrefix,
        &prefix,
        &text(&"é".repeat(1024))
    ));
    assert!(!ok(
        SettingKey::GitBranchPrefix,
        &prefix,
        &text(&format!("{}a", "é".repeat(1024)))
    ));
    // Control characters: only line breaks and tabs, only in instructions.
    assert!(ok(
        SettingKey::GitMessageInstructions,
        &text(""),
        &text("a\n\tb")
    ));
    assert!(!ok(
        SettingKey::GitMessageInstructions,
        &text(""),
        &text("a\rb")
    ));
    assert!(!ok(SettingKey::GitBranchPrefix, &prefix, &text("a\nb")));
    // Binding and consent keys: empty or one canonical UUID.
    let binding = Uuid::from_u128(0xabc5).to_string();
    for key in [
        SettingKey::UtilityAccountBinding,
        SettingKey::UtilityContentConsent,
        SettingKey::AutomaticReviewBinding,
        SettingKey::AutomaticReviewConsent,
    ] {
        assert!(ok(key, &text(""), &text("")));
        assert!(ok(key, &text(""), &text(&binding)));
        assert!(!ok(key, &text(""), &text(&binding.to_uppercase())));
        assert!(!ok(key, &text(""), &text("codex")));
    }
    assert_eq!(
        validate_value(SettingKey::EnergyConstrained, &flag, &count)
            .unwrap_err()
            .code,
        "settings.value_invalid"
    );
}

#[test]
fn text_values_that_cannot_be_shown_are_never_replaced() {
    assert_eq!(
        setting_text_view(SettingKey::GitBranchPrefix, ""),
        ValueView::Text(String::new())
    );
    assert_eq!(
        setting_text_view(SettingKey::GitMessageInstructions, "Use\nimperative"),
        ValueView::Text("Use\nimperative".into())
    );
    assert_eq!(
        setting_text_view(SettingKey::GitBranchPrefix, "jet/\n"),
        ValueView::Undisplayable
    );
    assert_eq!(
        setting_text_view(SettingKey::GitMessageInstructions, "\u{1b}[31m"),
        ValueView::Undisplayable
    );
    assert_eq!(
        setting_text_view(SettingKey::GitBranchPrefix, &"a".repeat(2_049)),
        ValueView::Undisplayable
    );
    assert_eq!(
        serde_json::to_value(ValueView::Undisplayable).unwrap(),
        json!({"type": "undisplayable"})
    );
    assert_eq!(
        serde_json::to_value(ValueView::Count(7)).unwrap(),
        json!({"type": "count", "value": 7})
    );
}

#[test]
fn every_source_maps_to_a_view() {
    let conversation = Uuid::from_u128(4);
    let cases = [
        (SettingSource::BuiltIn, json!({"source": "built_in"})),
        (
            SettingSource::Scope {
                scope: SettingScope::Plane,
            },
            json!({"source": "plane"}),
        ),
        (
            SettingSource::Scope {
                scope: SettingScope::Project {
                    project_id: PROJECT,
                },
            },
            json!({"source": "project", "projectId": PROJECT.to_string()}),
        ),
        (
            SettingSource::Scope {
                scope: SettingScope::Conversation {
                    conversation_id: conversation,
                },
            },
            json!({"source": "conversation", "conversationId": conversation.to_string()}),
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(
            serde_json::to_value(source_view(&source)).unwrap(),
            expected
        );
    }
    let view = resolved_view(&plane_set(
        SettingKey::EnergyConstrained,
        SettingValue::Flag(true),
    ));
    assert_eq!(
        serde_json::to_value(view).unwrap(),
        json!({
            "key": "energy.constrained",
            "value": {"type": "flag", "value": true},
            "source": {"source": "plane"}
        })
    );
}

#[test]
fn scope_and_change_inputs_are_typed() {
    assert_eq!(
        parse_scope(json!({"type": "plane"})).unwrap(),
        SettingScope::Plane
    );
    assert_eq!(
        parse_scope(json!({"type": "project", "project_id": PROJECT.to_string()})).unwrap(),
        SettingScope::Project {
            project_id: PROJECT
        }
    );
    for invalid in [
        json!({"type": "conversation", "conversation_id": PROJECT.to_string()}),
        json!({"type": "project", "project_id": "not-a-uuid"}),
        json!({"type": "project", "project_id": Uuid::from_u128(0xabc9).to_string().to_uppercase()}),
        json!({"type": "plane", "path": "/"}),
        json!("plane"),
    ] {
        assert_eq!(
            parse_scope(invalid).unwrap_err().code,
            "settings.scope_invalid"
        );
    }
    assert!(parse_change(json!({"kind": "clear", "value": 1})).is_err());
    assert!(matches!(
        parse_change(json!({"kind": "set", "value": {"type": "count", "value": 4}})).unwrap(),
        ChangeInput::Set {
            value: SettingValue::Count(4)
        }
    ));
}

// ---------------------------------------------------------------------------
// Ledger state machine (no I/O)
// ---------------------------------------------------------------------------

fn set_action(key: SettingKey) -> SettingsAction {
    SettingsAction::Set {
        key,
        scope: SettingScope::Plane,
        value: SettingValue::Flag(true),
    }
}

fn seen(key: SettingKey) -> Option<ResolvedSetting> {
    Some(built_in(key, SettingValue::Flag(false)))
}

fn send(begin: Begin) -> (PlaneKey, SettingsAction, Option<ResolvedSetting>) {
    match begin {
        Begin::Send {
            plane,
            action,
            guard,
        } => (plane, action, guard),
        Begin::Known(_) => panic!("expected a send"),
    }
}

fn begin_error(state: &SettingsState, id: Uuid, now: Instant) -> String {
    match state.begin(id, PlaneId::Local, now) {
        Ok(_) => panic!("expected a refusal"),
        Err(error) => error.code,
    }
}

#[test]
fn a_review_keeps_its_body_guards_only_its_first_attempt_and_replays_its_receipt() {
    let state = SettingsState::default();
    let now = Instant::now();
    let key = SettingKey::EnergyConstrained;
    let id = state
        .admit(local(), set_action(key), seen(key), now)
        .unwrap();

    let (plane, action, guard) = send(state.begin(id, PlaneId::Local, now).unwrap());
    assert_eq!(plane, local());
    assert_eq!(action, set_action(key));
    assert_eq!(guard, seen(key));

    state.mark_attempted(id).unwrap();
    // An uncertain retry resends the same body and skips the guard.
    let (_, action, guard) = send(state.begin(id, PlaneId::Local, now).unwrap());
    assert_eq!(action, set_action(key));
    assert_eq!(guard, None);
    // Nothing else may start on that slot meanwhile.
    assert_eq!(
        state
            .admit(local(), set_action(key), seen(key), now)
            .unwrap_err()
            .code,
        "settings.request_unresolved"
    );
    assert_eq!(
        state
            .ensure_slot_free(PlaneId::Local, set_action(key).slot())
            .unwrap_err()
            .code,
        "settings.request_unresolved"
    );
    // Another key, or the same key on another Plane, is another slot.
    state
        .admit(local(), set_action(SettingKey::DeveloperMode), None, now)
        .unwrap();
    state.admit(remote(2), set_action(key), None, now).unwrap();

    let receipt = SettingsReceipt::Applied {
        detail: AppliedDetail::Setting {
            key: key_spelling(key),
            value: Some(ValueView::Flag(true)),
        },
    };
    state.record(id, &receipt);
    assert!(matches!(
        state.begin(id, PlaneId::Local, now).unwrap(),
        Begin::Known(SettingsReceipt::Applied { .. })
    ));
    // Resolved: a new change may start.
    state
        .admit(local(), set_action(key), seen(key), now)
        .unwrap();
}

#[test]
fn overlapping_reviews_stay_valid_but_cannot_bypass_an_unresolved_one() {
    let state = SettingsState::default();
    let now = Instant::now();
    let key = SettingKey::EnergyConstrained;
    let first = state.admit(local(), set_action(key), None, now).unwrap();
    let second = state.admit(local(), set_action(key), None, now).unwrap();
    send(state.begin(second, PlaneId::Local, now).unwrap());
    state.mark_attempted(second).unwrap();
    assert_eq!(
        begin_error(&state, first, now),
        "settings.request_unresolved"
    );
    send(state.begin(second, PlaneId::Local, now).unwrap());
}

#[test]
fn unattempted_reviews_expire_and_only_they_are_evicted() {
    let state = SettingsState::default();
    let start = Instant::now();
    let key = SettingKey::EnergyConstrained;
    let stale = state.admit(local(), set_action(key), None, start).unwrap();
    let later = start + REVIEW_LIFETIME;
    assert_eq!(begin_error(&state, stale, later), "settings.review_expired");
    // Removed: asking again is still expired.
    assert_eq!(begin_error(&state, stale, start), "settings.review_expired");

    // An attempted review never expires and is never evicted.
    let attempted = state
        .admit(local(), set_action(SettingKey::DeveloperMode), None, start)
        .unwrap();
    state.mark_attempted(attempted).unwrap();
    let oldest = state.admit(local(), set_action(key), None, start).unwrap();
    for _ in 0..REVIEW_CAPACITY + 10 {
        state.admit(local(), set_action(key), None, start).unwrap();
    }
    assert_eq!(state.reviews.lock().unwrap().len(), REVIEW_CAPACITY);
    assert_eq!(
        begin_error(&state, oldest, start),
        "settings.review_expired"
    );
    send(
        state
            .begin(attempted, PlaneId::Local, later + REVIEW_LIFETIME)
            .unwrap(),
    );

    // Full of attempted reviews: the limit, never an eviction.
    let full = SettingsState::default();
    for n in 0..REVIEW_CAPACITY {
        let id = full
            .admit(remote(n as u128 + 10), set_action(key), None, start)
            .unwrap();
        full.mark_attempted(id).unwrap();
    }
    assert_eq!(
        full.admit(local(), set_action(key), None, start)
            .unwrap_err()
            .code,
        "settings.request_limit"
    );
}

#[test]
fn snapshots_expire_are_capped_and_belong_to_one_plane() {
    let state = SettingsState::default();
    let start = Instant::now();
    let values = vec![built_in(
        SettingKey::EnergyConstrained,
        SettingValue::Flag(false),
    )];
    let first = state
        .grant_snapshot(local(), SettingScope::Plane, values.clone(), start)
        .unwrap();
    assert!(state.snapshot(first, PlaneId::Local, start).is_ok());
    let error = state
        .snapshot(first, PlaneId::Remote(Uuid::from_u128(2)), start)
        .err()
        .unwrap();
    assert_eq!(error.code, "settings.snapshot_expired");
    assert!(state
        .snapshot(first, PlaneId::Local, start + SNAPSHOT_LIFETIME)
        .is_err());
    for n in 1..=SNAPSHOT_CAPACITY as u64 {
        state
            .grant_snapshot(
                local(),
                SettingScope::Plane,
                values.clone(),
                start + Duration::from_millis(n),
            )
            .unwrap();
    }
    assert_eq!(state.snapshots.lock().unwrap().len(), SNAPSHOT_CAPACITY);
    assert!(state.snapshot(first, PlaneId::Local, start).is_err());
}

// ---------------------------------------------------------------------------
// Commands without I/O
// ---------------------------------------------------------------------------

fn full_snapshot(plane: PlaneKey, scope: SettingScope, bridge: &JetBridge) -> Uuid {
    let values = SETTING_SCOPES
        .iter()
        .map(|(key, _)| built_in(*key, SettingValue::Flag(false)))
        .collect();
    bridge
        .settings
        .grant_snapshot(plane, scope, values, Instant::now())
        .unwrap()
}

#[tokio::test]
async fn scope_is_checked_from_the_table_before_any_io() {
    // The local Plane's socket does not exist: any I/O would be offline.
    let Setup { bridge, .. } = setup();
    let plane = full_snapshot(local(), SettingScope::Plane, &bridge);
    let project = full_snapshot(
        local(),
        SettingScope::Project {
            project_id: PROJECT,
        },
        &bridge,
    );
    let set = json!({"kind": "set", "value": {"type": "flag", "value": true}});
    let code = |result: Result<SettingChangePreparation, PublicError>| result.unwrap_err().code;
    let prepare = |snapshot: Uuid, key: &'static str, change: serde_json::Value| {
        let bridge = &bridge;
        async move {
            prepare_setting_change_for(bridge, "local", &snapshot.to_string(), key, change).await
        }
    };
    assert_eq!(
        code(prepare(plane, "git.auto_push", set.clone()).await),
        "settings.scope_invalid"
    );
    assert_eq!(
        code(prepare(project, "review.automatic", set.clone()).await),
        "settings.scope_invalid"
    );
    assert_eq!(
        code(prepare(plane, "git.nope", set.clone()).await),
        "settings.key_invalid"
    );
    assert_eq!(
        code(
            prepare(
                plane,
                "energy.constrained",
                json!({"kind": "set", "value": {"type": "count", "value": 1}})
            )
            .await
        ),
        "settings.value_invalid"
    );

    // A key the snapshot (the Plane's minor) does not name.
    let older = bridge
        .settings
        .grant_snapshot(
            local(),
            SettingScope::Plane,
            vec![built_in(
                SettingKey::EnergyConstrained,
                SettingValue::Flag(false),
            )],
            Instant::now(),
        )
        .unwrap();
    assert_eq!(
        code(prepare(older, "review.automatic", set).await),
        "settings.key_unavailable"
    );
}

#[tokio::test]
async fn load_rejects_a_conversation_scope_and_an_unknown_plane_without_io() {
    let Setup { bridge, .. } = setup();
    let error = load_settings_for(
        &bridge,
        "local",
        json!({"type": "conversation", "conversation_id": PROJECT.to_string()}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "settings.scope_invalid");
    let error = load_settings_for(
        &bridge,
        &Uuid::from_u128(77).to_string(),
        json!({"type": "plane"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "plane.unknown");
    let error = load_work_context_for(&bridge, "remote").await.unwrap_err();
    assert_eq!(error.code, "plane.unknown");
}

#[tokio::test]
async fn a_snapshot_or_review_of_one_plane_is_refused_through_another() {
    let Setup {
        bridge, directory, ..
    } = setup();
    let (a, b) = (Uuid::from_u128(0xa), Uuid::from_u128(0xb));
    // Neither socket exists: reaching either would be `transport.offline`.
    for id in [a, b] {
        bridge.planes.insert_for_test(
            id,
            "Plane",
            PlaneClient::new(
                directory.path().join(format!("{id}.sock")),
                CLIENT,
                [Duration::from_millis(1)],
                Duration::from_millis(1),
            ),
        );
    }
    let binding_a = PlaneBinding {
        plane: PlaneId::Remote(a),
        identity: None,
    };
    let snapshot = full_snapshot(binding_a, SettingScope::Plane, &bridge);
    let error = prepare_setting_change_for(
        &bridge,
        &b.to_string(),
        &snapshot.to_string(),
        "energy.constrained",
        json!({"kind": "clear"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "settings.snapshot_expired");

    let review = bridge
        .settings
        .admit(
            binding_a,
            set_action(SettingKey::EnergyConstrained),
            None,
            Instant::now(),
        )
        .unwrap();
    let receipt = apply_settings_change_for(&bridge, &b.to_string(), &review.to_string())
        .await
        .unwrap();
    let SettingsReceipt::Refused { error } = receipt else {
        panic!("expected a refusal");
    };
    assert_eq!(error.code, "settings.review_expired");
    // Still unattempted on A: nothing was sent anywhere.
    assert!(!bridge.settings.reviews.lock().unwrap()[&review].attempted);
}

// ---------------------------------------------------------------------------
// Fake jetd (wave 3.2 §4.8 cases 1-7)
// ---------------------------------------------------------------------------

struct Fake {
    setup: Setup,
    plane_id: String,
    listener: UnixListener,
}

fn fake_plane() -> Fake {
    let setup = setup();
    let socket = setup.directory.path().join("fake-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0xfa4e);
    setup.bridge.planes.insert_for_test(
        id,
        "Fake Plane",
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
        listener,
    }
}

const KEY: SettingKey = SettingKey::RetentionTrashGraceDays;

fn grace(days: u32, source: SettingSource) -> ResolvedSetting {
    resolved(KEY, SettingValue::Count(days), source)
}

fn plane_snapshot(cursor: u64, settings: Vec<ResolvedSetting>) -> SettingSnapshot {
    SettingSnapshot {
        cursor,
        scope: SettingScope::Plane,
        settings,
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

/// Serves `status` then `Settings{All}` on one connection.
async fn serve_load(listener: &UnixListener, plane: PlaneStatus, snapshot: SettingSnapshot) {
    let (mut reader, mut writer) = accept(listener, CLIENT).await;
    let (stream, message) = next_message(&mut reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::Status,
            ..
        }
    ));
    answer(&mut writer, stream, &message, QueryResponse::Status(plane)).await;
    let (stream, message) = next_message(&mut reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::Settings {
                scope: SettingScope::Plane,
                selection: SettingSelection::All,
            },
            ..
        }
    ));
    answer(
        &mut writer,
        stream,
        &message,
        QueryResponse::Settings(snapshot),
    )
    .await;
}

/// Expects a `Settings{Key}` read for `KEY` and answers with `setting`.
async fn serve_key(reader: &mut Reader, writer: &mut Writer, setting: ResolvedSetting) {
    let (stream, message) = next_message(reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::Settings {
                scope: SettingScope::Plane,
                selection: SettingSelection::Key { key: KEY },
            },
            ..
        }
    ));
    answer(
        writer,
        stream,
        &message,
        QueryResponse::Settings(plane_snapshot(20, vec![setting])),
    )
    .await;
}

/// The next request must be exactly `SetSetting{KEY, Plane, Count(days)}`
/// under `command_id`.
async fn expect_set(reader: &mut Reader, command_id: Uuid, days: u32) -> (StreamId, ClientMessage) {
    let (stream, message) = next_message(reader).await;
    match &message {
        ClientMessage::Command {
            command_id: sent,
            command:
                CommandRequest::SetSetting {
                    key: KEY,
                    scope: SettingScope::Plane,
                    value: SettingValue::Count(value),
                },
            ..
        } if *sent == command_id && *value == days => {}
        other => panic!("unexpected request {other:?}"),
    }
    (stream, message)
}

async fn assert_closed(reader: &mut Reader) {
    assert!(reader.read().await.is_err(), "no further request expected");
}

async fn set_applied(writer: &mut Writer, stream: StreamId, message: &ClientMessage, days: u32) {
    reply(
        writer,
        stream,
        ServerMessage::CommandResult {
            id: request_id(message),
            result: CommandResponse::SettingSet {
                key: KEY,
                scope: SettingScope::Plane,
                value: SettingValue::Count(days),
            },
        },
    )
    .await;
}

async fn load(fake: &Fake) -> serde_json::Value {
    let view = load_settings_for(&fake.setup.bridge, &fake.plane_id, json!({"type": "plane"}))
        .await
        .unwrap();
    serde_json::to_value(view).unwrap()
}

async fn prepare(fake: &Fake, snapshot_id: &str, days: u32) -> serde_json::Value {
    let preparation = prepare_setting_change_for(
        &fake.setup.bridge,
        &fake.plane_id,
        snapshot_id,
        key_spelling(KEY),
        json!({"kind": "set", "value": {"type": "count", "value": days}}),
    )
    .await
    .unwrap();
    serde_json::to_value(preparation).unwrap()
}

async fn apply(fake: &Fake, review_id: &str) -> Result<serde_json::Value, PublicError> {
    apply_settings_change_for(&fake.setup.bridge, &fake.plane_id, review_id)
        .await
        .map(|receipt| serde_json::to_value(receipt).unwrap())
}

fn review_id(preparation: &serde_json::Value) -> String {
    assert_eq!(preparation["kind"], "review", "{preparation}");
    preparation["review"]["reviewId"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn loaded_id(view: &serde_json::Value) -> String {
    view["snapshotId"].as_str().unwrap().to_owned()
}

/// Loads, prepares a change to `days`, and returns the review ID.
async fn reviewed(fake: &Fake, seen: &ResolvedSetting, days: u32) -> (serde_json::Value, String) {
    let (view, preparation) = tokio::join!(
        async {
            let view = load(fake).await;
            let preparation = prepare(fake, &loaded_id(&view), days).await;
            (view, preparation)
        },
        async {
            serve_load(
                &fake.listener,
                status(10),
                plane_snapshot(10, vec![seen.clone()]),
            )
            .await;
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            serve_key(&mut reader, &mut writer, seen.clone()).await;
            assert_closed(&mut reader).await;
        }
    )
    .0;
    let id = review_id(&preparation);
    (view, id)
}

#[tokio::test]
async fn case_1_prepare_and_apply_read_fresh_and_send_under_the_review_id() {
    let fake = fake_plane();
    let seen = grace(30, SettingSource::BuiltIn);
    let (view, preparation) = tokio::join!(
        async {
            let view = load(&fake).await;
            let preparation = prepare(&fake, &loaded_id(&view), 14).await;
            (view, preparation)
        },
        async {
            serve_load(
                &fake.listener,
                status(10),
                plane_snapshot(10, vec![seen.clone()]),
            )
            .await;
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            serve_key(&mut reader, &mut writer, seen.clone()).await;
            // The prepare sent no Command.
            assert_closed(&mut reader).await;
        }
    )
    .0;
    assert_eq!(view["cursor"], "10");
    assert_eq!(
        view["settings"][0],
        json!({"key": "retention.trash_grace_days", "value": {"type": "count", "value": 30}, "source": {"source": "built_in"}})
    );
    assert_eq!(
        preparation["review"]["after"],
        json!({"type": "count", "value": 14})
    );
    assert_eq!(
        preparation["review"]["subject"],
        json!({"kind": "setting", "key": "retention.trash_grace_days", "scope": {"type": "plane"}})
    );
    let id = review_id(&preparation);
    let command_id = Uuid::parse_str(&id).unwrap();
    let (receipt, ()) = tokio::join!(apply(&fake, &id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_key(&mut reader, &mut writer, seen.clone()).await;
        let (stream, message) = expect_set(&mut reader, command_id, 14).await;
        set_applied(&mut writer, stream, &message, 14).await;
    });
    assert_eq!(
        receipt.unwrap(),
        json!({"kind": "applied", "detail": {"kind": "setting", "key": "retention.trash_grace_days", "value": {"type": "count", "value": 14}}})
    );
    // A lost IPC reply replays the receipt without any I/O.
    assert_eq!(apply(&fake, &id).await.unwrap()["kind"], "applied");
}

#[tokio::test]
async fn case_2_an_uncertain_send_is_retried_without_the_guard_and_with_the_same_body() {
    let fake = fake_plane();
    let seen = grace(30, SettingSource::BuiltIn);
    let (_, id) = reviewed(&fake, &seen, 14).await;
    let command_id = Uuid::parse_str(&id).unwrap();

    let (first, sent) = tokio::join!(apply(&fake, &id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_key(&mut reader, &mut writer, seen.clone()).await;
        let (_, message) = expect_set(&mut reader, command_id, 14).await;
        // The connection drops before the reply.
        drop((reader, writer));
        message
    });
    let error = first.unwrap_err();
    assert_eq!(error.code, "transport.offline");
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));

    let (second, resent) = tokio::join!(apply(&fake, &id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        // No Settings query this time: straight to the same Command.
        let (stream, message) = expect_set(&mut reader, command_id, 14).await;
        set_applied(&mut writer, stream, &message, 14).await;
        message
    });
    assert_eq!(second.unwrap()["kind"], "applied");
    let body = |message: &ClientMessage| match message {
        ClientMessage::Command {
            command_id,
            command,
            ..
        } => serde_json::to_value((command_id, command)).unwrap(),
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(body(&sent), body(&resent));
}

#[tokio::test]
async fn case_3_a_changed_value_at_prepare_admits_nothing() {
    let fake = fake_plane();
    let seen = grace(30, SettingSource::BuiltIn);
    let elsewhere = grace(
        45,
        SettingSource::Scope {
            scope: SettingScope::Plane,
        },
    );
    let preparation = tokio::join!(
        async {
            let view = load(&fake).await;
            prepare(&fake, &loaded_id(&view), 14).await
        },
        async {
            serve_load(
                &fake.listener,
                status(10),
                plane_snapshot(10, vec![seen.clone()]),
            )
            .await;
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            serve_key(&mut reader, &mut writer, elsewhere.clone()).await;
            assert_closed(&mut reader).await;
        }
    )
    .0;
    assert_eq!(
        preparation,
        json!({"kind": "changed", "current": {"key": "retention.trash_grace_days", "value": {"type": "count", "value": 45}, "source": {"source": "plane"}}})
    );
    assert!(fake
        .setup
        .bridge
        .settings
        .reviews
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn case_4_a_changed_value_at_first_apply_sends_nothing_and_drops_the_review() {
    let fake = fake_plane();
    let seen = grace(30, SettingSource::BuiltIn);
    let elsewhere = grace(
        45,
        SettingSource::Scope {
            scope: SettingScope::Plane,
        },
    );
    let (_, id) = reviewed(&fake, &seen, 14).await;
    let (receipt, ()) = tokio::join!(apply(&fake, &id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_key(&mut reader, &mut writer, elsewhere.clone()).await;
        assert_closed(&mut reader).await;
    });
    let receipt = receipt.unwrap();
    assert_eq!(receipt["kind"], "changed");
    assert_eq!(
        receipt["current"]["value"],
        json!({"type": "count", "value": 45})
    );
    let again = apply(&fake, &id).await.unwrap();
    assert_eq!(again["kind"], "refused");
    assert_eq!(again["error"]["code"], "settings.review_expired");
}

#[tokio::test]
async fn case_5_a_definite_refusal_ends_the_review_and_a_new_prepare_is_allowed() {
    let fake = fake_plane();
    let seen = grace(30, SettingSource::BuiltIn);
    let (view, id) = reviewed(&fake, &seen, 0).await;
    let command_id = Uuid::parse_str(&id).unwrap();
    let (receipt, ()) = tokio::join!(apply(&fake, &id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_key(&mut reader, &mut writer, seen.clone()).await;
        let (stream, message) = expect_set(&mut reader, command_id, 0).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::InvalidInput,
            "setting.value_below_minimum",
        )
        .await;
    });
    let receipt = receipt.unwrap();
    assert_eq!(receipt["kind"], "refused");
    assert_eq!(receipt["error"]["code"], "setting.value_below_minimum");
    assert_eq!(receipt["error"]["planeId"], fake.plane_id.as_str());
    assert!(!receipt.to_string().contains("daemon text"));

    let snapshot_id = loaded_id(&view);
    let (again, ()) = tokio::join!(prepare(&fake, &snapshot_id, 7), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_key(&mut reader, &mut writer, seen.clone()).await;
    });
    assert_ne!(review_id(&again), id);
}

#[tokio::test]
async fn case_6_a_degraded_audit_refuses_the_change_and_is_reported_by_load() {
    let fake = fake_plane();
    let seen = grace(30, SettingSource::BuiltIn);
    let degraded = PlaneStatus {
        security: Some(SecurityState::Degraded {
            breach: AuditBreach::HeadMissing,
            epoch: 2,
            head: None,
            store_sequence: 9,
        }),
        ..status(10)
    };
    let (view, preparation) = tokio::join!(
        async {
            let view = load(&fake).await;
            let preparation = prepare(&fake, &loaded_id(&view), 14).await;
            (view, preparation)
        },
        async {
            serve_load(
                &fake.listener,
                degraded,
                plane_snapshot(10, vec![seen.clone()]),
            )
            .await;
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            serve_key(&mut reader, &mut writer, seen.clone()).await;
        }
    )
    .0;
    assert_eq!(
        view["planeState"],
        json!({"security": "degraded", "recovery": "unknown"})
    );
    assert!(!view.to_string().contains("head_missing"));
    let id = review_id(&preparation);
    let command_id = Uuid::parse_str(&id).unwrap();
    let (receipt, ()) = tokio::join!(apply(&fake, &id), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_key(&mut reader, &mut writer, seen.clone()).await;
        let (stream, message) = expect_set(&mut reader, command_id, 14).await;
        refuse(
            &mut writer,
            stream,
            &message,
            ErrorCategory::Conflict,
            "security.audit_degraded",
        )
        .await;
    });
    let receipt = receipt.unwrap();
    assert_eq!(receipt["kind"], "refused");
    assert_eq!(receipt["error"]["code"], "security.audit_degraded");
    assert_eq!(receipt["error"]["category"], "conflict");
}

#[tokio::test]
async fn case_7_read_only_recovery_is_reported_by_load() {
    let fake = fake_plane();
    let read_only = PlaneStatus {
        security: Some(SecurityState::Trusted),
        recovery: Some(RecoveryStatus {
            state: RecoveryState::ReadOnly,
            reason: Some(jet_protocol::RecoveryReason::IntegrityCheckFailed),
            snapshots: Vec::new(),
            deletion_ledger: None,
        }),
        ..status(10)
    };
    let (view, ()) = tokio::join!(
        load(&fake),
        serve_load(&fake.listener, read_only, plane_snapshot(10, Vec::new()))
    );
    assert_eq!(
        view["planeState"],
        json!({"security": "trusted", "recovery": "read_only"})
    );
}

#[tokio::test]
async fn work_context_names_projects_without_their_roots_and_reports_partial_failures() {
    let fake = fake_plane();
    let (view, ()) = tokio::join!(
        load_work_context_for(&fake.setup.bridge, &fake.plane_id),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::Status(status(10)),
            )
            .await;
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::Projects,
                    ..
                }
            ));
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::Projects(jet_protocol::ProjectList {
                    cursor: 12,
                    projects: vec![jet_protocol::Project {
                        project_id: PROJECT,
                        root: "/home/someone/private/jet".into(),
                        registered_by: Actor::InteractiveClient { client_id: CLIENT },
                        registered_at_unix_ms: 1,
                    }],
                }),
            )
            .await;
            let (stream, message) = next_message(&mut reader).await;
            refuse(
                &mut writer,
                stream,
                &message,
                ErrorCategory::Unavailable,
                "account.store_unavailable",
            )
            .await;
            let (stream, message) = next_message(&mut reader).await;
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::AutodeleteRules(jet_protocol::AutodeleteRules {
                    cursor: 12,
                    rules: Vec::new(),
                }),
            )
            .await;
        }
    );
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(
        view["projects"],
        json!([{"id": PROJECT.to_string(), "name": "jet"}])
    );
    assert!(!view.to_string().contains("/home/someone"));
    assert_eq!(view["projectsCursor"], "12");
    assert_eq!(view["bindings"], json!([]));
    assert_eq!(view["autodelete"], json!({"count": 0}));
    assert_eq!(view["issues"][0]["section"], "accounts");
    assert_eq!(
        view["issues"][0]["error"]["code"],
        "account.store_unavailable"
    );
}

// ---------------------------------------------------------------------------
// Change watcher (§4.5)
// ---------------------------------------------------------------------------

fn event(kind: &str, payload: serde_json::Value) -> Event {
    Event {
        sequence: 21,
        event_id: Uuid::from_u128(21),
        actor: Actor::InteractiveClient { client_id: CLIENT },
        origin: None,
        recorded_at_unix_ms: 1,
        conversation_id: Some(Uuid::from_u128(2)),
        run_id: Some(Uuid::from_u128(3)),
        kind: kind.into(),
        payload_version: 1,
        payload,
    }
}

fn change_for(event: Event) -> Option<serde_json::Value> {
    settings_change(
        NativeUpdate::Event(EventSummary::from_event(event)),
        PlaneId::Local,
    )
    .map(|change| serde_json::to_value(change).unwrap())
}

#[test]
fn a_timeline_event_never_reaches_the_settings_window() {
    let turn = event(
        "turn.input",
        json!({"turn_id": Uuid::from_u128(9), "text": "private prompt"}),
    );
    assert_eq!(change_for(turn), None);
    assert_eq!(change_for(event("run.output", json!({}))), None);
    assert_eq!(change_for(event("approval.requested", json!({}))), None);
}

#[test]
fn a_setting_event_forwards_only_its_identifiers() {
    let change = change_for(event(
        "setting.changed",
        json!({
            "key": "git.auto_push",
            "scope": {"type": "project", "project_id": PROJECT},
            "value": {"type": "flag", "value": true},
            "note": "extra"
        }),
    ))
    .unwrap();
    assert_eq!(
        change,
        json!({
            "type": "change",
            "sequence": "21",
            "kind": "setting.changed",
            "settingKey": "git.auto_push",
            "settingScope": "project",
            "projectId": PROJECT.to_string()
        })
    );
    let keys: Vec<&String> = change.as_object().unwrap().keys().collect();
    for forbidden in ["timeline", "conversationId", "runId", "text", "value"] {
        assert!(!keys.iter().any(|key| *key == forbidden), "{forbidden}");
    }
    assert!(!change.to_string().contains("extra"));

    let cleared = change_for(event(
        "setting.cleared",
        json!({"key": "energy.constrained", "scope": {"type": "plane"}}),
    ))
    .unwrap();
    assert_eq!(cleared["settingScope"], "plane");
    assert_eq!(cleared["projectId"], serde_json::Value::Null);

    let hostile = change_for(event(
        "setting.changed",
        json!({"key": "<script>", "scope": {"type": "plane"}}),
    ))
    .unwrap();
    assert_eq!(hostile["settingKey"], serde_json::Value::Null);
    assert_eq!(hostile["kind"], "setting.changed");
    assert!(!hostile.to_string().contains("script"));

    let usage = change_for(event("usage.recorded", json!({"tokens": 5}))).unwrap();
    assert_eq!(usage["kind"], "usage.recorded");
    assert_eq!(usage["settingKey"], serde_json::Value::Null);
}

#[test]
fn watcher_failures_name_their_plane() {
    let plane = PlaneId::Remote(Uuid::from_u128(4));
    let change = settings_change(
        NativeUpdate::Reconnecting {
            error: PublicError::offline(),
        },
        plane,
    )
    .unwrap();
    let json = serde_json::to_value(change).unwrap();
    assert_eq!(json["type"], "reconnecting");
    assert_eq!(json["error"]["planeId"], plane.to_string());
    assert_eq!(
        serde_json::to_value(settings_change(NativeUpdate::Resumed { after: 5 }, plane).unwrap())
            .unwrap(),
        json!({"type": "resumed", "after": "5"})
    );
}

/// Lets the runtime process an abort; no wall-clock sleep.
async fn finished(task: &tokio::task::AbortHandle) -> bool {
    for _ in 0..1_000 {
        if task.is_finished() {
            return true;
        }
        tokio::task::yield_now().await;
    }
    task.is_finished()
}

#[tokio::test]
async fn one_watcher_runs_at_a_time_and_window_close_stops_it() {
    let fake = fake_plane();
    let bridge = &fake.setup.bridge;
    assert_eq!(
        watch_settings_changes_for(bridge, &fake.plane_id, "x".into(), |_| true)
            .unwrap_err()
            .code,
        "event.cursor_invalid"
    );
    watch_settings_changes_for(bridge, &fake.plane_id, "4".into(), |_| true).unwrap();
    let first = bridge.settings.watch.task.lock().unwrap().clone().unwrap();
    watch_settings_changes_for(bridge, &fake.plane_id, "4".into(), |_| true).unwrap();
    let second = bridge.settings.watch.task.lock().unwrap().clone().unwrap();
    assert!(second.0 > first.0);
    assert!(finished(&first.1).await);
    bridge.settings_window_closed();
    assert!(bridge.settings.watch.task.lock().unwrap().is_none());
    assert!(finished(&second.1).await);
}

#[tokio::test]
async fn the_watcher_forwards_only_staleness_events_from_the_journal() {
    let fake = fake_plane();
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = received.clone();
    watch_settings_changes_for(
        &fake.setup.bridge,
        &fake.plane_id,
        "20".into(),
        move |change| {
            sink.lock()
                .unwrap()
                .push(serde_json::to_value(change).unwrap());
            true
        },
    )
    .unwrap();
    let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
    let (stream, message) = next_message(&mut reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::Events { after: 20 },
            ..
        }
    ));
    let mut turn = event(
        "turn.input",
        json!({"turn_id": Uuid::from_u128(9), "text": "secret"}),
    );
    let mut setting = event(
        "setting.changed",
        json!({"key": "energy.constrained", "scope": {"type": "plane"}, "value": {"type": "flag", "value": true}}),
    );
    turn.sequence = 21;
    setting.sequence = 22;
    answer(
        &mut writer,
        stream,
        &message,
        QueryResponse::Events(jet_protocol::EventPage {
            cursor: 22,
            events: vec![turn, setting],
        }),
    )
    .await;
    // The next poll proves both events were handled.
    let _ = next_message(&mut reader).await;
    let received = received.lock().unwrap().clone();
    assert_eq!(received.len(), 2, "{received:?}");
    assert_eq!(received[0], json!({"type": "resumed", "after": "20"}));
    assert_eq!(received[1]["sequence"], "22");
    assert_eq!(received[1]["settingKey"], "energy.constrained");
    assert!(!serde_json::to_string(&received).unwrap().contains("secret"));
    fake.setup.bridge.settings_window_closed();
}
