//! The Wave 3.3 manual checks (`docs/wave-3.3.md`, "Verification"), driven
//! through the shell's own command functions against a scratch `jetd`.
//!
//! It is ignored by default because it needs a built `jetd`. From
//! `apps/jet-tauri`:
//!
//! ```sh
//! cargo build --manifest-path ../../packages/Cargo.toml -p jet-daemon --bin jetd
//! JET_E2E_JETD=$(realpath ../../packages/target/debug/jetd) cargo test \
//!     --manifest-path src-tauri/Cargo.toml live_system -- --ignored --nocapture
//! ```
//!
//! The Plane lives in a new directory under `$XDG_RUNTIME_DIR` (Unix socket
//! paths are limited to 108 bytes) and is deleted afterwards. `~/.jet` and
//! the login keyring are never touched. To reach read-only Recovery mode the
//! test stops `jetd`, damages the scratch store and starts `jetd` again.
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jet_protocol::{RetentionPolicy, WorkingTreeRequest};
use serde::Serialize;
use serde_json::{json as value, Value};
use uuid::Uuid;

use super::{audit, errors::PublicError, retention, system, JetBridge};

const PLANE: &str = "local";

struct Daemon {
    jetd: PathBuf,
    home: PathBuf,
    log: PathBuf,
    child: Option<Child>,
}

impl Daemon {
    fn start(&mut self) {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .expect("jetd log");
        let child = Command::new(&self.jetd)
            .arg("serve")
            .arg("--home")
            .arg(self.home.join(".jet"))
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("log"))
            .stderr(log)
            .spawn()
            .expect("jetd starts");
        self.child = Some(child);
        let socket = self.home.join(".jet/runtime/jetd.sock");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !socket.exists() {
            assert!(
                Instant::now() < deadline,
                "jetd did not open its socket:\n{}",
                std::fs::read_to_string(&self.log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// SIGTERM, then wait, so the store is closed cleanly.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(child.id().to_string())
                .status();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if child.try_wait().expect("wait").is_some() {
                    break;
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = std::fs::remove_file(self.home.join(".jet/runtime/jetd.sock"));
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

fn json(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("serializes")
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{pointer} missing in {value}"))
        .to_owned()
}

fn ok<T: Serialize>(label: &str, result: Result<T, PublicError>) -> Value {
    match result {
        Ok(view) => json(&view),
        Err(error) => panic!("{label}: {} {}", error.code, error.message),
    }
}

fn step(name: &str, outcome: impl std::fmt::Display) {
    println!("[wave-3.3] {name}: {outcome}");
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Forgets the cached connection so the next request reconnects.
async fn reconnect(bridge: &JetBridge) {
    let (_, client) = bridge.plane(Some(PLANE)).expect("local Plane");
    client.reset().await;
    if let Ok(session) = client.connect().await {
        client.invalidate(&session).await;
    }
}

async fn create_task(bridge: &JetBridge) -> String {
    let (_, client) = bridge.plane(Some(PLANE)).expect("local Plane");
    let connection = client.connect().await.expect("connects");
    let conversation = connection
        .create_conversation_in(
            Uuid::new_v4(),
            RetentionPolicy::Retain,
            WorkingTreeRequest::NoProject,
        )
        .await
        .expect("task created");
    conversation.conversation_id.to_string()
}

async fn health(bridge: &JetBridge) -> Value {
    ok("health", system::load_health(bridge, PLANE, false).await)
}

/// Forget in Jet (or Delete everywhere) through a fresh review.
async fn move_to_trash(bridge: &JetBridge, task: &str, mode: &str) -> Value {
    let preview = ok(
        "preview",
        retention::preview_trash_for(bridge, PLANE, task).await,
    );
    let review = text(&preview, "/reviewId");
    let mode = serde_json::from_value(value!(mode)).expect("mode");
    ok(
        "trash",
        retention::trash_conversation_for(bridge, PLANE, &review, mode, false).await,
    )
}

fn in_trash(trash: &Value, task: &str) -> bool {
    trash["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .any(|entry| entry["conversationId"] == task)
}

async fn trash_and_restore(bridge: &JetBridge, task: &str, round: u32) {
    let trashed = move_to_trash(bridge, task, "forget").await;
    assert_eq!(trashed["kind"], "trashed", "{trashed}");
    let trash = ok("trash list", retention::load_trash_for(bridge, PLANE).await);
    assert!(in_trash(&trash, task), "{trash}");
    let status = ok(
        "trash status",
        retention::load_trash_status_for(bridge, PLANE, task).await,
    );
    let restored = ok(
        "restore",
        retention::restore_conversation_for(bridge, PLANE, task).await,
    );
    let trash = ok("trash list", retention::load_trash_for(bridge, PLANE).await);
    assert!(!in_trash(&trash, task), "{trash}");
    step(
        &format!("forget, then restore (round {round})"),
        format!("trashed {trashed}; status {status}; restored {restored}"),
    );
}

async fn change_rule(bridge: &JetBridge, change: Value) -> Value {
    let change = serde_json::from_value(change).expect("change");
    ok(
        "rule change",
        retention::autodelete::change_rule_for(bridge, PLANE, change).await,
    )
}

async fn rules(bridge: &JetBridge) -> Value {
    ok(
        "rules",
        retention::autodelete::load_rules_for(bridge, PLANE).await,
    )
}

async fn recovery_action(bridge: &JetBridge, action: Value) -> Result<Value, PublicError> {
    let action = serde_json::from_value(action).expect("action");
    let review = system::recovery::prepare(bridge, PLANE, action).await?;
    let review = json(&review);
    step("recovery review", &review);
    let review_id = text(&review, "/reviewId");
    system::recovery::execute(bridge, PLANE, &review_id)
        .await
        .map(|outcome| json(&outcome))
}

async fn export(bridge: &JetBridge, target: &Path) -> Value {
    let (binding, _) = bridge.plane(Some(PLANE)).expect("local Plane");
    ok(
        "export",
        audit::export_to(bridge, binding, target, "e2e", unix_ms()).await,
    )
}

fn check_evidence(target: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(target)
        .expect("evidence")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "evidence is owner-only");
    let body = std::fs::read_to_string(target).expect("evidence");
    let header: Value = serde_json::from_str(body.lines().next().expect("header")).expect("json");
    assert_eq!(header["format"], "jet-audit-evidence", "{header}");
    let directory = target.parent().expect("directory");
    let partials = std::fs::read_dir(directory)
        .expect("directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
        .count();
    assert_eq!(partials, 0, "no partial file is left behind");
    step(
        "audit evidence",
        format!(
            "{} lines, mode {mode:o}, header {header}",
            body.lines().count()
        ),
    );
}

/// Zeroes the pages of the `conversations` table, so the next open fails
/// `PRAGMA quick_check` while the schema still loads, and jetd serves the
/// store read-only. Needs the `sqlite3` command-line shell (with `dbstat`).
fn damage_store(jet_home: &Path) {
    let store = jet_home.join("plane.sqlite3");
    for suffix in ["-wal", "-shm"] {
        let mut journal = store.as_os_str().to_owned();
        journal.push(suffix);
        assert!(
            !PathBuf::from(journal).exists(),
            "jetd left a {suffix} file"
        );
    }
    let uri = format!("file:{}?mode=ro", store.display());
    let output = Command::new("sqlite3")
        .arg(&uri)
        .arg("pragma page_size; select pageno from dbstat where name = 'conversations';")
        .output()
        .expect("sqlite3 runs");
    assert!(output.status.success(), "sqlite3 failed");
    let numbers: Vec<u64> = String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .map(|line| line.trim().parse().expect("a number"))
        .collect();
    let (page_size, pages) = numbers.split_first().expect("page size");
    assert!(!pages.is_empty(), "the conversations table has pages");
    let mut file = OpenOptions::new().write(true).open(&store).expect("store");
    for page in pages {
        file.seek(SeekFrom::Start((page - 1) * page_size))
            .expect("seek");
        file.write_all(&vec![0; usize::try_from(*page_size).expect("size")])
            .expect("damage");
    }
    file.sync_all().expect("sync");
    step("damaged store pages", format!("{pages:?}"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a built jetd: set JET_E2E_JETD"]
async fn wave_3_3_manual_checks_against_a_scratch_plane() {
    let jetd = PathBuf::from(std::env::var("JET_E2E_JETD").expect("JET_E2E_JETD is unset"));
    let jetd = jetd.canonicalize().expect("jetd path");
    let runtime = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let root = tempfile::Builder::new()
        .prefix("jet33.")
        .tempdir_in(runtime)
        .expect("scratch root");
    let home = root.path().join("h");
    std::fs::create_dir_all(home.join(".jet")).expect("home");
    let mut daemon = Daemon {
        jetd,
        home: home.clone(),
        log: root.path().join("jetd.log"),
        child: None,
    };
    daemon.start();
    let bridge = JetBridge::for_local_plane(&home, &root.path().join("app")).expect("bridge opens");

    // Jet Trash: forget and restore twice, then delete everywhere.
    let first = create_task(&bridge).await;
    trash_and_restore(&bridge, &first, 1).await;
    trash_and_restore(&bridge, &first, 2).await;
    let second = create_task(&bridge).await;
    let deleted = move_to_trash(&bridge, &second, "delete_everywhere").await;
    assert_eq!(deleted["entry"]["reason"], "delete_everywhere", "{deleted}");
    step("delete everywhere on an idle task", &deleted);

    // Auto-delete with drafting off (the default), then days and approval.
    let compiled = change_rule(
        &bridge,
        value!({"kind": "compile", "rule_id": null, "prompt": "Tasks nobody touched for a month"}),
    )
    .await;
    step("compile with drafting off", &compiled);
    let read = rules(&bridge).await;
    assert_eq!(
        read["rules"][0]["state"]["reason"], "utility.disabled",
        "{read}"
    );
    step("rules after compile", &read);
    let rule_id = text(&read, "/rules/0/ruleId");
    let days = change_rule(
        &bridge,
        value!({"kind": "set_inactive_days", "rule_id": rule_id, "inactive_days": 30}),
    )
    .await;
    step("set days", &days);
    let read = rules(&bridge).await;
    step("rules after days", &read);
    let token = text(&read, "/rules/0/state/approveToken");
    let approved = change_rule(&bridge, value!({"kind": "approve", "token_id": token})).await;
    step("approve", &approved);
    let read = rules(&bridge).await;
    assert_eq!(read["rules"][0]["state"]["kind"], "approved", "{read}");
    step("rules after approve", &read);

    // Health, audit paging and evidence.
    let serving = health(&bridge).await;
    step("health (serving)", &serving["recovery"]);
    let page = ok(
        "audit page",
        audit::load_page(&bridge, PLANE, None, false).await,
    );
    step(
        "audit first page",
        format!(
            "{} records, complete {}",
            page["entries"].as_array().map_or(0, Vec::len),
            page["complete"]
        ),
    );
    let evidence = root.path().join("evidence").join("audit.jsonl");
    std::fs::create_dir_all(evidence.parent().expect("parent")).expect("evidence dir");
    step("export", export(&bridge, &evidence).await);
    check_evidence(&evidence);

    // Wait for the day's snapshot the writes above asked for.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut snapshots = serving["recovery"]["snapshotCount"].as_u64().unwrap_or(0);
    while snapshots == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
        snapshots = health(&bridge).await["recovery"]["snapshotCount"]
            .as_u64()
            .unwrap_or(0);
    }
    step("snapshots before purge", snapshots);

    // Purge on the scratch Plane.
    let purged = recovery_action(&bridge, value!({"kind": "purge_snapshots"}))
        .await
        .unwrap_or_else(|error| panic!("purge: {} {}", error.code, error.message));
    assert_eq!(purged["kind"], "purged", "{purged}");
    step("purge", &purged);
    // A write after the purge so the store has something to lose.
    let third = create_task(&bridge).await;
    step("task created after purge", &third);
    let deadline = Instant::now() + Duration::from_secs(30);
    while health(&bridge).await["recovery"]["snapshots"]
        .as_array()
        .is_none_or(Vec::is_empty)
    {
        assert!(Instant::now() < deadline, "no snapshot to restore");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // Read-only Recovery mode on the scratch store, then restore.
    daemon.stop();
    damage_store(&home.join(".jet"));
    daemon.start();
    reconnect(&bridge).await;
    let damaged = health(&bridge).await;
    step("health after damage", &damaged["recovery"]);
    if damaged["recovery"]["kind"] != "read_only" {
        panic!("the damaged store is not read-only: {damaged}");
    }
    let snapshot = text(&damaged, "/recovery/snapshots/0/snapshotId");
    let restored = recovery_action(
        &bridge,
        value!({"kind": "restore_snapshot", "snapshot_id": snapshot}),
    )
    .await;
    let restored =
        restored.unwrap_or_else(|error| panic!("restore: {} {}", error.code, error.message));
    assert_eq!(restored["kind"], "restored", "{restored}");
    step("restore", &restored);
    let after = health(&bridge).await;
    assert_eq!(after["recovery"]["kind"], "serving", "{after}");
    step("health after restore", &after["recovery"]);
    step("security after restore", &after["security"]);
    let trash = ok(
        "trash list",
        retention::load_trash_for(&bridge, PLANE).await,
    );
    step(
        "trash after restore",
        format!(
            "{} entries",
            trash["entries"].as_array().map_or(0, Vec::len)
        ),
    );

    // A degraded audit after the store moved backwards: save the evidence,
    // then begin a new epoch.
    // The audit head beside the store is not restored (docs/recovery.md).
    assert_eq!(after["security"]["kind"], "degraded", "{after}");
    {
        let refused = recovery_action(&bridge, value!({"kind": "begin_audit_epoch"})).await;
        let code = refused.map_or_else(|error| error.code, |outcome| outcome.to_string());
        assert_eq!(
            code, "audit.export_required",
            "the epoch waits for evidence"
        );
        step("epoch before export", code);
        let evidence = root.path().join("evidence").join("degraded.jsonl");
        step("export (degraded)", export(&bridge, &evidence).await);
        check_evidence(&evidence);
        let begun = recovery_action(&bridge, value!({"kind": "begin_audit_epoch"}))
            .await
            .unwrap_or_else(|error| panic!("epoch: {} {}", error.code, error.message));
        assert_eq!(begun["kind"], "epoch_begun", "{begun}");
        step("begin audit epoch", &begun);
        let security = health(&bridge).await["security"].clone();
        assert_eq!(security["kind"], "trusted", "{security}");
        step("security after epoch", &security);
    }
    daemon.stop();
}
