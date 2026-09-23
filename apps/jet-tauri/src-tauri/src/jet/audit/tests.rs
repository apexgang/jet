use std::time::Duration;

use jet_protocol::{
    AuditActor, AuditBreach, AuditEntry, AuditOutcome, AuditRisk, AuditTarget, ClientMessage,
    ErrorCategory, PlaneStatus, QueryRequest, QueryResponse, SecurityAudit, SecurityState,
    ServerMessage, StreamId, WireError,
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
    planes::PlaneId,
};

type Reader = jet_protocol::FrameReader<OwnedReadHalf>;
type Writer = jet_protocol::FrameWriter<OwnedWriteHalf>;

const CLIENT: Uuid = Uuid::from_u128(0xa0d1);
const OTHER: Uuid = Uuid::from_u128(0xa0d2);
const IDENTITY: Uuid = Uuid::from_u128(0xfeed);
const TARGET: Uuid = Uuid::from_u128(0x7a26);
const REFERENCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn entry(sequence: u64, actor: AuditActor) -> AuditEntry {
    AuditEntry {
        sequence,
        epoch: 2,
        record_id: Uuid::from_u128(u128::from(sequence)),
        recorded_at_unix_ms: 1_700_000_000_000,
        plane_id: IDENTITY,
        actor,
        target: AuditTarget {
            kind: "account_binding".into(),
            reference: REFERENCE.into(),
            identity: Some(TARGET.to_string()),
        },
        decision: "account.bound".into(),
        risk: AuditRisk::Elevated,
        outcome: AuditOutcome::Succeeded,
    }
}

fn mine(sequence: u64) -> AuditEntry {
    entry(
        sequence,
        AuditActor::InteractiveClient { client_id: CLIENT },
    )
}

fn page(cursor: u64, sequences: impl IntoIterator<Item = u64>) -> SecurityAudit {
    SecurityAudit {
        cursor,
        entries: sequences.into_iter().map(mine).collect(),
    }
}

fn degraded(epoch: u64) -> SecurityState {
    SecurityState::Degraded {
        breach: AuditBreach::HeadNotInStore,
        epoch,
        head: None,
        store_sequence: 9,
    }
}

fn status(security: Option<SecurityState>) -> PlaneStatus {
    PlaneStatus {
        cursor: Some(9),
        plane_id: IDENTITY,
        daemon_starts: 3,
        started_at_unix_ms: 1_700_000_000_000,
        core_version: "1.43.0".into(),
        security,
        recovery: None,
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

#[test]
fn identifiers_are_withheld_by_default_and_the_plane_id_never_crosses() {
    let hidden = serde_json::to_value(entry_view(&mine(7), CLIENT, false)).unwrap();
    assert_eq!(
        hidden,
        json!({
            "sequence": "7",
            "epoch": "2",
            "recordedAtUnixMs": "1700000000000",
            "actor": {"kind": "this_device", "clientId": null},
            "target": {"kind": "account_binding", "identity": null, "reference": null},
            "decision": "account.bound",
            "risk": "elevated",
            "outcome": "succeeded",
        })
    );
    assert!(!hidden.to_string().contains(&IDENTITY.to_string()));

    let shown = serde_json::to_value(entry_view(&mine(7), OTHER, true)).unwrap();
    assert_eq!(
        shown["actor"],
        json!({"kind": "other_client", "clientId": CLIENT.to_string()})
    );
    assert_eq!(shown["target"]["identity"], TARGET.to_string());
    assert_eq!(shown["target"]["reference"], "0123456789abcdef…");
    assert!(!shown.to_string().contains(&IDENTITY.to_string()));
}

#[test]
fn revealed_identifiers_keep_only_allowlisted_shapes() {
    let mut odd = entry(1, AuditActor::CraftRevocation);
    odd.target.identity = Some("/home/user/.ssh/id_ed25519".into());
    odd.target.reference = "NOT-HEX".into();
    odd.target.kind = "Account Binding".into();
    odd.decision = "<script>".into();
    odd.risk = AuditRisk::Destructive;
    odd.outcome = AuditOutcome::Denied;
    let view = serde_json::to_value(entry_view(&odd, CLIENT, true)).unwrap();
    assert_eq!(
        view["actor"],
        json!({"kind": "craft_revocation", "clientId": null})
    );
    assert_eq!(
        view["target"],
        json!({"kind": "unknown", "identity": null, "reference": null})
    );
    assert_eq!(view["decision"], "unknown");
    assert_eq!(view["risk"], "destructive");
    assert_eq!(view["outcome"], "denied");

    let mut retention = entry(2, AuditActor::Retention);
    retention.target.identity = None;
    retention.target.reference = "abc".into();
    retention.outcome = AuditOutcome::Failed;
    retention.risk = AuditRisk::Routine;
    let view = serde_json::to_value(entry_view(&retention, CLIENT, true)).unwrap();
    assert_eq!(view["actor"]["kind"], "retention");
    assert_eq!(view["target"]["reference"], "abc");
    assert_eq!(view["risk"], "routine");
    assert_eq!(view["outcome"], "failed");

    assert_eq!(shown_reference(""), None);
    assert_eq!(shown_reference(&"a".repeat(65)), None);
    assert_eq!(shown_reference("ABCDEF"), None);
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[test]
fn a_page_is_complete_when_empty_or_when_it_reaches_the_cursor() {
    assert!(complete(&page(0, [])));
    assert!(complete(&page(9, [])));
    assert!(complete(&page(3, [1, 2, 3])));
    assert!(!complete(&page(9, [1, 2, 3])));
    let view = serde_json::to_value(page_view(&page(9, [4, 5]), CLIENT, false)).unwrap();
    assert_eq!(view["cursor"], "9");
    assert_eq!(view["complete"], false);
    assert_eq!(view["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn a_page_out_of_order_past_its_cursor_or_oversized_is_refused() {
    assert!(check_page(0, &page(3, [1, 2, 3])).is_ok());
    for (after, bad) in [
        (0, page(3, [2, 1])),
        (0, page(3, [1, 1])),
        (2, page(3, [2, 3])),
        (0, page(3, [1, 4])),
        (0, page(300, 1..=257)),
    ] {
        let error = check_page(after, &bad).unwrap_err();
        assert_eq!(error.code, "audit.page_invalid");
        assert_eq!(error.category, "invalid_response");
    }
}

#[test]
fn the_starting_position_is_a_bounded_decimal() {
    assert_eq!(parse_after(None).unwrap(), 0);
    assert_eq!(parse_after(Some("42".into())).unwrap(), 42);
    for bad in [
        "",
        "-1",
        "1e3",
        "１",
        "123456789012345678901",
        "18446744073709551616",
    ] {
        assert_eq!(
            parse_after(Some(bad.into())).unwrap_err().code,
            "audit.cursor_invalid",
            "{bad}"
        );
    }
}

// ---------------------------------------------------------------------------
// File names
// ---------------------------------------------------------------------------

#[test]
fn the_slug_keeps_only_lowercase_ascii_digits_and_single_dashes() {
    assert_eq!(slug("Build box"), "build-box");
    assert_eq!(slug("  ../..//etc/passwd  "), "etc-passwd");
    assert_eq!(slug("Мой ноутбук"), "plane");
    assert_eq!(slug("Café #2 — Linux"), "caf-2-linux");
    assert_eq!(slug(""), "plane");
    assert_eq!(slug("---"), "plane");
    let long = slug(&"ab-".repeat(40));
    assert!(long.len() <= MAX_SLUG);
    assert!(!long.ends_with('-'));
}

#[test]
fn file_names_carry_the_utc_date() {
    assert_eq!(utc_date(0), (1970, 1, 1));
    assert_eq!(utc_date(1_709_164_800_000), (2024, 2, 29));
    assert_eq!(utc_date(1_709_251_199_999), (2024, 2, 29));
    assert_eq!(utc_date(1_735_689_600_000), (2025, 1, 1));
    assert_eq!(utc_date(-1), (1969, 12, 31));
    assert_eq!(
        file_name("build-box", 1_709_164_800_000),
        "jet-audit-build-box-2024-02-29.jsonl"
    );
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
    let socket = setup.directory.path().join("audit-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0xa0d0);
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
        plane_client(self.bridge(), &self.plane_id).unwrap().0
    }

    async fn export(&self, target: &Path) -> Result<AuditExportView, PublicError> {
        export_to(
            self.bridge(),
            self.binding(),
            target,
            "build-box",
            1_709_164_800_000,
        )
        .await
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

/// Answers the next audit read, which must start after `after`.
async fn serve_page(reader: &mut Reader, writer: &mut Writer, after: u64, page: SecurityAudit) {
    let (stream, message) = expect_audit(reader, after).await;
    reply(
        writer,
        stream,
        ServerMessage::QueryResult {
            id: request_id(&message),
            result: QueryResponse::SecurityAudit(page),
        },
    )
    .await;
}

async fn expect_audit(reader: &mut Reader, after: u64) -> (StreamId, ClientMessage) {
    let (stream, message) = next_message(reader).await;
    match &message {
        ClientMessage::Query {
            query: QueryRequest::SecurityAudit { after: sent },
            ..
        } if *sent == after => {}
        other => panic!("expected an audit read after {after}, got {other:?}"),
    }
    (stream, message)
}

#[tokio::test]
async fn a_page_names_this_device_and_a_refusal_names_its_plane() {
    let fake = fake_plane();
    let (view, ()) = tokio::join!(
        load_page(fake.bridge(), &fake.plane_id, Some("4".into()), false),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let mut other = entry(6, AuditActor::InteractiveClient { client_id: OTHER });
            other.recorded_at_unix_ms = 1;
            serve_page(
                &mut reader,
                &mut writer,
                4,
                SecurityAudit {
                    cursor: 6,
                    entries: vec![mine(5), other],
                },
            )
            .await;
        }
    );
    let value = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(value["complete"], true);
    assert_eq!(value["entries"][0]["actor"]["kind"], "this_device");
    assert_eq!(value["entries"][1]["actor"]["kind"], "other_client");
    assert!(!value.to_string().contains(&IDENTITY.to_string()));

    // Paired clients may not read the owner-only audit.
    let (denied, ()) = tokio::join!(
        load_page(fake.bridge(), &fake.plane_id, None, true),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = expect_audit(&mut reader, 0).await;
            reply(
                &mut writer,
                stream,
                ServerMessage::Error {
                    id: Some(request_id(&message)),
                    error: WireError {
                        category: ErrorCategory::Unauthorized,
                        code: "unauthorized".into(),
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
    );
    let denied = denied.unwrap_err();
    assert_eq!(denied.category, "unauthorized");
    assert_eq!(denied.plane_id.as_deref(), Some(fake.plane_id.as_str()));

    // Validated before any I/O.
    let bad = load_page(fake.bridge(), &fake.plane_id, Some("x".into()), false)
        .await
        .unwrap_err();
    assert_eq!(bad.code, "audit.cursor_invalid");
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_export_writes_a_header_and_every_record_owner_only_and_marks_the_epoch() {
    let fake = fake_plane();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("evidence.jsonl");
    let (saved, ()) = tokio::join!(fake.export(&target), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_status(&mut reader, &mut writer, status(Some(degraded(2)))).await;
        serve_page(&mut reader, &mut writer, 0, page(5, [1, 2, 3])).await;
        serve_page(&mut reader, &mut writer, 3, page(5, [4, 5])).await;
    });
    assert_eq!(
        saved.unwrap(),
        AuditExportView::Saved {
            records: "5".into(),
            file_name: "evidence.jsonl".into(),
        }
    );

    let written = std::fs::read_to_string(&target).unwrap();
    let lines: Vec<serde_json::Value> = written
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 6);
    assert_eq!(
        lines[0],
        json!({
            "format": "jet-audit-evidence",
            "version": 1,
            "plane_label": "build-box",
            "security": serde_json::to_value(degraded(2)).unwrap(),
            "exported_at_unix_ms": 1_709_164_800_000_i64,
        })
    );
    // Each record is the wire form, oldest first.
    let first: AuditEntry = serde_json::from_value(lines[1].clone()).unwrap();
    assert_eq!(first, mine(1));
    assert_eq!(lines[5]["sequence"], "5");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(!directory.path().join(".evidence.jsonl.partial").exists());
    assert_eq!(
        fake.bridge().audit.mark(fake.plane, IDENTITY).unwrap(),
        Some(ExportMark {
            identity: IDENTITY,
            epoch: Some(2),
            through: 5,
        })
    );
    // Another Plane identity at the same handle has no mark.
    assert_eq!(
        fake.bridge()
            .audit
            .mark(fake.plane, Uuid::from_u128(1))
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn an_export_of_a_trusted_audit_unlocks_no_epoch() {
    let fake = fake_plane();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("trusted.jsonl");
    let (saved, ()) = tokio::join!(fake.export(&target), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_status(
            &mut reader,
            &mut writer,
            status(Some(SecurityState::Trusted)),
        )
        .await;
        serve_page(&mut reader, &mut writer, 0, page(0, [])).await;
    });
    assert!(matches!(saved.unwrap(), AuditExportView::Saved { ref records, .. } if records == "0"));
    assert_eq!(
        fake.bridge()
            .audit
            .mark(fake.plane, IDENTITY)
            .unwrap()
            .unwrap()
            .epoch,
        None
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap().lines().count(), 1);
}

#[tokio::test]
async fn an_existing_partial_file_or_symlink_is_never_followed() {
    let fake = fake_plane();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("evidence.jsonl");
    std::fs::write(&target, "keep me").unwrap();
    let elsewhere = directory.path().join("elsewhere");
    std::fs::write(&elsewhere, "untouched").unwrap();
    let partial = directory.path().join(".evidence.jsonl.partial");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&elsewhere, &partial).unwrap();
    #[cfg(not(unix))]
    std::fs::write(&partial, "existing").unwrap();

    let (failed, ()) = tokio::join!(fake.export(&target), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_status(&mut reader, &mut writer, status(Some(degraded(2)))).await;
    });
    let failed = failed.unwrap_err();
    assert_eq!(failed.code, "audit.export_failed");
    assert_eq!(failed.category, "internal");
    assert!(failed.retryable);
    assert_eq!(failed.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "untouched");
    // The existing partial path is left as it was.
    assert!(std::fs::symlink_metadata(&partial).is_ok());
    assert_eq!(
        fake.bridge().audit.mark(fake.plane, IDENTITY).unwrap(),
        None
    );
}

#[tokio::test]
async fn a_failed_export_removes_its_partial_file_and_leaves_the_target() {
    let fake = fake_plane();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("evidence.jsonl");
    std::fs::write(&target, "older evidence").unwrap();
    let (failed, ()) = tokio::join!(fake.export(&target), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_status(&mut reader, &mut writer, status(Some(degraded(2)))).await;
        serve_page(&mut reader, &mut writer, 0, page(9, [1, 2])).await;
        // The connection drops before the second page.
        expect_audit(&mut reader, 2).await;
    });
    assert_eq!(failed.unwrap_err().category, "offline");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "older evidence");
    assert!(!directory.path().join(".evidence.jsonl.partial").exists());
    assert_eq!(
        fake.bridge().audit.mark(fake.plane, IDENTITY).unwrap(),
        None
    );

    // An invalid page also ends the export without a file.
    let fresh = directory.path().join("fresh.jsonl");
    let (failed, ()) = tokio::join!(fake.export(&fresh), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        serve_status(&mut reader, &mut writer, status(None)).await;
        serve_page(&mut reader, &mut writer, 0, page(3, [2, 1])).await;
    });
    assert_eq!(failed.unwrap_err().code, "audit.page_invalid");
    assert!(!fresh.exists());
    assert!(!directory.path().join(".fresh.jsonl.partial").exists());
}

#[tokio::test]
async fn one_export_per_plane_at_a_time() {
    let fake = fake_plane();
    let first = enter_export(fake.bridge(), fake.plane).unwrap();
    let busy = enter_export(fake.bridge(), fake.plane).err().unwrap();
    assert_eq!(busy.code, "audit.export_busy");
    assert_eq!(busy.category, "conflict");
    assert_eq!(busy.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    // Another Plane is not blocked.
    assert!(enter_export(fake.bridge(), PlaneId::Local).is_ok());
    drop(first);
    assert!(enter_export(fake.bridge(), fake.plane).is_ok());
}

#[test]
fn a_mark_is_cleared_with_its_plane_only() {
    let state = AuditState::default();
    let mark = ExportMark {
        identity: IDENTITY,
        epoch: Some(1),
        through: 3,
    };
    state.record(PlaneId::Local, mark).unwrap();
    let remote = PlaneId::Remote(Uuid::from_u128(5));
    state.record(remote, mark).unwrap();
    state.clear(PlaneId::Local);
    assert_eq!(state.mark(PlaneId::Local, IDENTITY).unwrap(), None);
    assert_eq!(state.mark(remote, IDENTITY).unwrap(), Some(mark));
}
