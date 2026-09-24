//! The Wave 3.1 manual end-to-end matrix, driven through the shell's own
//! flows against real `jetd` Planes over the system `ssh`.
//!
//! It is ignored by default because it needs the sandbox that
//! `scripts/remote-plane-sandbox.sh` builds (an unprivileged sshd, two jetd
//! Planes and a private keyring). From `apps/jet-tauri`:
//!
//! ```sh
//! scripts/remote-plane-sandbox.sh <jetd> -- cargo test \
//!     --manifest-path src-tauri/Cargo.toml live_e2e -- --ignored --nocapture
//! ```
//!
//! Three bridges stand in for three computers. `owner` is a client on the
//! remote Plane's own computer (its local Plane is the remote Plane), `app`
//! is this app enrolling over SSH, and `third` / `fresh` are further
//! enrolling computers. Webview-only behavior (feeds, Recent, Search,
//! notifications) is not reachable from here and is covered elsewhere.
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;

use super::{enrollment, errors::PublicError, pairing, JetBridge};

struct Sandbox {
    root: PathBuf,
    destination: String,
    local_home: PathBuf,
    remote_home: PathBuf,
    spawn_log: PathBuf,
    known_hosts: PathBuf,
    ctl: PathBuf,
}

impl Sandbox {
    fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var(name)
                .unwrap_or_else(|_| panic!("{name} is unset: run inside remote-plane-sandbox.sh"))
        };
        Self {
            root: var("JET_E2E_ROOT").into(),
            destination: var("JET_E2E_DESTINATION"),
            local_home: var("JET_E2E_LOCAL_HOME").into(),
            remote_home: var("JET_E2E_REMOTE_HOME").into(),
            spawn_log: var("JET_E2E_SPAWN_LOG").into(),
            known_hosts: var("JET_E2E_KNOWN_HOSTS").into(),
            ctl: var("JET_E2E_CTL").into(),
        }
    }

    fn bridge(&self, home: &Path, name: &str) -> JetBridge {
        JetBridge::for_local_plane(home, &self.root.join("apps").join(name)).expect("bridge opens")
    }

    /// The arguments of every ssh this process spawned. Lines are `time
    /// parent arguments`; jetd's own `ssh -V` tool check is not ours.
    fn spawn_lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.spawn_log)
            .expect("spawn log")
            .lines()
            .filter_map(|line| {
                let (_, rest) = line.split_once(' ').expect("timestamped");
                let (parent, arguments) = rest.split_once(' ').expect("parent");
                (parent != "jetd").then(|| arguments.to_owned())
            })
            .collect()
    }

    fn spawns(&self) -> usize {
        self.spawn_lines().len()
    }

    fn ctl(&self, action: &str) {
        let status = Command::new(&self.ctl)
            .arg(action)
            .status()
            .expect("sandbox control");
        assert!(status.success(), "sandbox control {action} failed");
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

fn code<T: std::fmt::Debug>(result: Result<T, PublicError>) -> String {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(error) => error.code,
    }
}

fn step(number: &str, outcome: &str) {
    println!("[e2e] {number}: {outcome}");
}

/// The remote Plane's registry handle in `bridge` (the only remote entry).
fn remote_id(bridge: &JetBridge) -> String {
    let planes = json(&bridge.planes.snapshot(None).expect("planes"));
    planes["planes"]
        .as_array()
        .expect("planes")
        .iter()
        .find(|plane| plane["kind"] == "remote")
        .map(|plane| text(plane, "/planeId"))
        .expect("a remote Plane")
}

fn connection(bridge: &JetBridge, plane_id: &str) -> Value {
    let planes = json(&bridge.planes.snapshot(None).expect("planes"));
    planes["planes"]
        .as_array()
        .expect("planes")
        .iter()
        .find(|plane| plane["planeId"] == plane_id)
        .map(|plane| plane["connection"].clone())
        .expect("the Plane is listed")
}

/// Drops the cached ssh session so the next request spawns ssh again.
async fn drop_session(bridge: &JetBridge, plane_id: &str) {
    let (_, client) = bridge.plane(Some(plane_id)).expect("known Plane");
    client.reset().await;
    if let Ok(session) = client.connect().await {
        client.invalidate(&session).await;
    }
}

/// Runs one owner-confirmed pairing: `owner` opens an offer on `owner_plane`,
/// `claimant` claims it for `draft`, `owner` types the string the claimant
/// computed (jetd compares it with its own), and `claimant` finishes.
async fn pair(
    owner: &JetBridge,
    owner_plane: &str,
    claimant: &JetBridge,
    draft: &Value,
    confirm_after: Duration,
    wrong_first: bool,
) -> Value {
    assert_eq!(draft["kind"], "pairing_required", "{draft}");
    let draft_id = text(draft, "/draftId");
    pairing::set_gate(owner, owner_plane, "open")
        .await
        .expect("gate opens");
    let offer = json(
        &pairing::open_offer(owner, owner_plane)
            .await
            .expect("offer opens"),
    );
    let code = text(&offer, "/code");
    let offer_id = text(&offer, "/offerId");
    let claimed = json(
        &enrollment::claim(claimant, &draft_id, &code, Instant::now())
            .await
            .expect("claim"),
    );
    let shown = text(&claimed, "/authenticationString");
    let waiting = json(&pairing::load(owner, owner_plane).await.expect("pairing"));
    assert_eq!(
        text(&waiting, "/pending/progress/kind"),
        "awaiting_confirmation",
        "{waiting}"
    );
    assert_eq!(
        text(&waiting, "/pending/progress/clientId"),
        claimant.planes.local().client_id().to_string()
    );
    if wrong_first {
        let mut wrong = shown.clone().into_bytes();
        let last = wrong.len() - 1;
        wrong[last] = b'0' + (wrong[last] - b'0' + 1) % 10;
        let wrong = String::from_utf8(wrong).expect("ascii");
        let refused = pairing::confirm(owner, owner_plane, &offer_id, &wrong).await;
        step(
            "2",
            &format!(
                "a wrong authentication string is refused: {}",
                refused
                    .as_ref()
                    .map_or_else(|error| error.code.clone(), |view| json(view).to_string())
            ),
        );
        assert!(
            refused.is_err() || json(&refused.unwrap())["progress"]["kind"] != "confirmed",
            "a wrong string must not confirm"
        );
    }
    if !confirm_after.is_zero() {
        step("2", &format!("waiting {confirm_after:?} before confirming"));
        tokio::time::sleep(confirm_after).await;
    }
    let confirmed = json(
        &pairing::confirm(owner, owner_plane, &offer_id, &shown)
            .await
            .expect("the string this computer computed matches jetd's"),
    );
    assert_eq!(text(&confirmed, "/progress/kind"), "confirmed");
    let plane = json(
        &enrollment::complete(claimant, &text(&claimed, "/ticketId"), Instant::now())
            .await
            .expect("finish"),
    );
    pairing::set_gate(owner, owner_plane, "closed")
        .await
        .expect("gate closes");
    assert_eq!(plane["connection"]["state"], "online", "{plane}");
    plane
}

async fn change(bridge: &JetBridge, plane: &str, client: uuid::Uuid, change: &str) -> Value {
    let review = json(
        &pairing::prepare_change(bridge, plane, &client.to_string(), change)
            .await
            .expect("review"),
    );
    let receipt = pairing::execute_change(
        bridge,
        text(&review, "/reviewId").parse().expect("review ID"),
    )
    .await
    .expect("receipt");
    json(&receipt)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/remote-plane-sandbox.sh"]
async fn live_e2e_remote_plane_matrix() {
    let sandbox = Sandbox::from_env();
    let destination = sandbox.destination.as_str();
    // jetd gives the owner 2 minutes from the claim (PAIRING_WINDOW_MS), so
    // the late confirm lands at 100 s, near the end of that window. The spec
    // said 3 minutes; that is past the window and ends in
    // `pairing.offer_expired`.
    let confirm_after = Duration::from_secs(
        std::env::var("JET_E2E_CONFIRM_AFTER_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(100),
    );
    let owner = sandbox.bridge(&sandbox.remote_home, "owner");
    let app = sandbox.bridge(&sandbox.local_home, "app");
    let app_client = app.planes.local().client_id();

    // 1 + 2. Owner controls on the remote computer's local Plane; this app
    // enrolls over SSH and confirms late.
    let owner_status = json(&owner.planes.snapshot(None).expect("owner planes"));
    let draft = json(
        &enrollment::add(&app, destination, None, Instant::now())
            .await
            .expect("add"),
    );
    let plane = pair(&owner, "local", &app, &draft, confirm_after, true).await;
    let remote = text(&plane, "/planeId");
    let owner_pairing = json(&pairing::load(&owner, "local").await.expect("pairing"));
    let identity = text(&plane, "/planeIdentity");
    step(
        "1+2",
        &format!(
            "paired as {app_client}; Plane identity {identity}; owner sees {} paired client(s); \
             owner local label {}",
            owner_pairing["clients"].as_array().map_or(0, Vec::len),
            owner_status["planes"][0]["label"]
        ),
    );

    // 3. This app opens an offer on the remote Plane over SSH (an owner
    // operation by a remote client) and a third computer pairs through it.
    let third = sandbox.bridge(&sandbox.local_home, "third");
    let third_draft = json(
        &enrollment::add(&third, destination, None, Instant::now())
            .await
            .expect("third add"),
    );
    let third_plane = pair(&app, &remote, &third, &third_draft, Duration::ZERO, false).await;
    step(
        "3",
        &format!(
            "third computer paired through an offer this app opened remotely; same Plane \
             identity: {}",
            third_plane["planeIdentity"] == plane["planeIdentity"]
        ),
    );
    assert_eq!(third_plane["planeIdentity"], plane["planeIdentity"]);

    // 4. Disable, enable, revoke, pair again.
    let receipt = change(&owner, "local", app_client, "disable").await;
    assert_eq!(receipt["kind"], "applied", "{receipt}");
    drop_session(&app, &remote).await;
    let denied = code(pairing::load(&app, &remote).await);
    let spawned = sandbox.spawns();
    let again = code(pairing::load(&app, &remote).await);
    let again_twice = code(pairing::load(&app, &remote).await);
    step(
        "4",
        &format!(
            "disabled: {denied}, then {again}/{again_twice} with {} new ssh spawn(s); state {}",
            sandbox.spawns() - spawned,
            connection(&app, &remote)["state"]
        ),
    );
    assert_eq!(denied, "connection.unauthorized");
    assert_eq!(sandbox.spawns(), spawned, "a sticky denial spawns no ssh");
    let receipt = change(&owner, "local", app_client, "enable").await;
    assert_eq!(receipt["kind"], "applied", "{receipt}");
    let retried = json(
        &enrollment::repair(&app, &remote, None, Instant::now())
            .await
            .expect("retry"),
    );
    assert_eq!(retried["kind"], "connected", "{retried}");
    pairing::load(&app, &remote)
        .await
        .expect("online after enable");
    step("4", "enabled, Retry logs in again: online");
    let receipt = change(&owner, "local", app_client, "revoke").await;
    assert_eq!(receipt["kind"], "applied", "{receipt}");
    drop_session(&app, &remote).await;
    let revoked = code(pairing::load(&app, &remote).await);
    assert_eq!(revoked, "connection.unauthorized");
    let draft = json(
        &enrollment::repair(&app, &remote, None, Instant::now())
            .await
            .expect("pair again"),
    );
    let repaired = pair(&owner, "local", &app, &draft, Duration::ZERO, false).await;
    step(
        "4",
        &format!(
            "revoked: {revoked}; Pair again kept the Plane handle: {}",
            repaired["planeId"] == plane["planeId"]
        ),
    );
    assert_eq!(repaired["planeId"], plane["planeId"]);

    // 5. Revoke this computer over its own remote connection, then Forget.
    let receipt = change(&app, &remote, app_client, "revoke").await;
    step(
        "5",
        &format!(
            "self-revoke receipt {}; state {}",
            receipt["kind"],
            connection(&app, &remote)
        ),
    );
    assert!(
        receipt["kind"] == "applied_unverified" || receipt["kind"] == "applied",
        "{receipt}"
    );
    let forgotten = json(&enrollment::forget(&app, &remote).await.expect("forget"));
    assert!(forgotten["planes"]
        .as_array()
        .expect("planes")
        .iter()
        .all(|plane| plane["kind"] == "local"));
    step("5", "Forget removed the Plane from this app");

    // Forget, then Add again: the app pairs with the same Plane again.
    let draft = json(
        &enrollment::add(&app, destination, None, Instant::now())
            .await
            .expect("add again"),
    );
    pair(&owner, "local", &app, &draft, Duration::ZERO, false).await;
    let app_remote = remote_id(&app);
    step("5", "Add a Plane after Forget pairs again");

    // The failure steps use the third computer, which was never revoked:
    // see the jetd restart finding below.
    let remote = remote_id(&third);

    // 6. jetd stops on the remote computer.
    sandbox.ctl("stop-remote");
    drop_session(&third, &remote).await;
    let stopped = code(pairing::load(&third, &remote).await);
    step(
        "6",
        &format!(
            "jetd stopped: {stopped}; state {}",
            connection(&third, &remote)["state"]
        ),
    );
    assert_eq!(stopped, "plane.jetd_unavailable");
    sandbox.ctl("start-remote");
    drop_session(&third, &remote).await;
    pairing::load(&third, &remote)
        .await
        .expect("online after jetd starts");
    // Backend finding: a client that was revoked and then paired again is
    // refused after jetd restarts (its row is gone). Recorded, not asserted.
    drop_session(&app, &app_remote).await;
    step(
        "6",
        &format!(
            "after the jetd restart the re-paired app gets {:?}",
            pairing::load(&app, &app_remote)
                .await
                .map(|_| "online")
                .map_err(|error| error.code)
        ),
    );

    // 7. The host key is no longer trusted.
    let trusted = std::fs::read(&sandbox.known_hosts).expect("known_hosts");
    std::fs::write(&sandbox.known_hosts, b"").expect("clear known_hosts");
    drop_session(&third, &remote).await;
    let untrusted = code(pairing::load(&third, &remote).await);
    let written = std::fs::read(&sandbox.known_hosts).expect("known_hosts");
    step(
        "7",
        &format!(
            "unknown host key: {untrusted}; known_hosts still empty: {}",
            written.is_empty()
        ),
    );
    assert_eq!(untrusted, "ssh.connection_failed");
    assert!(written.is_empty(), "ssh wrote to known_hosts");
    std::fs::write(&sandbox.known_hosts, trusted).expect("restore known_hosts");
    drop_session(&third, &remote).await;
    pairing::load(&third, &remote)
        .await
        .expect("online with the host key trusted again");

    // 8. A locked keyring: the unlock happens before ssh starts. With no
    // display the prompt cannot be answered, which is a dismissal.
    sandbox.ctl("lock-keyring");
    drop_session(&third, &remote).await;
    let spawned = sandbox.spawns();
    let locked = code(pairing::load(&third, &remote).await);
    step(
        "8",
        &format!(
            "keyring locked: {locked}; ssh spawns during the attempt: {}",
            sandbox.spawns() - spawned
        ),
    );
    assert_eq!(locked, "identity.secret_store_locked");
    assert_eq!(sandbox.spawns(), spawned, "ssh started before the unlock");
    sandbox.ctl("stop-keyring");
    sandbox.ctl("start-keyring");
    drop_session(&third, &remote).await;
    pairing::load(&third, &remote)
        .await
        .expect("online after unlocking");
    step("8", "unlocked (daemon restarted with its password): online");

    // 9. No Secret Service: session-only pairing, then a durable one.
    sandbox.ctl("stop-keyring");
    let fresh = sandbox.bridge(&sandbox.local_home, "fresh");
    let fresh_client = fresh.planes.local().client_id();
    let unavailable = code(enrollment::add(&fresh, destination, None, Instant::now()).await);
    assert_eq!(unavailable, "identity.secret_store_unavailable");
    let draft = json(
        &enrollment::add(&fresh, destination, Some(true), Instant::now())
            .await
            .expect("session-only add"),
    );
    let session_plane = pair(&owner, "local", &fresh, &draft, Duration::ZERO, false).await;
    assert_eq!(session_plane["credential"], "session", "{session_plane}");
    pairing::load(&app, "local")
        .await
        .expect("the local Plane works without a keyring");
    drop(fresh);
    let restarted = sandbox.bridge(&sandbox.local_home, "fresh");
    let fresh_remote = remote_id(&restarted);
    let ended = code(pairing::load(&restarted, &fresh_remote).await);
    step(
        "9",
        &format!("no keyring: {unavailable}; session-only paired; after restart: {ended}"),
    );
    assert_eq!(ended, "identity.session_ended");
    sandbox.ctl("start-keyring");
    let draft = json(
        &enrollment::repair(&restarted, &fresh_remote, None, Instant::now())
            .await
            .expect("pair again durably"),
    );
    let durable = if draft["kind"] == "pairing_required" {
        // The session key is gone; the Plane still lists this client with
        // that key, so the owner revokes it before pairing again.
        let revoked = change(&owner, "local", fresh_client, "revoke").await;
        step(
            "9",
            &format!("owner revoked the ended session key: {revoked}"),
        );
        pair(&owner, "local", &restarted, &draft, Duration::ZERO, false).await
    } else {
        draft["plane"].clone()
    };
    step(
        "9",
        &format!("keyring back: Pair again is {}", durable["credential"]),
    );
    assert_eq!(durable["credential"], "durable", "{durable}");

    // Every ssh spawn used the fixed argument list.
    let lines = sandbox.spawn_lines();
    let fixed = format!(
        "-T -a -S none -o StrictHostKeyChecking=yes -o VerifyHostKeyDNS=no -o BatchMode=yes \
         -o ConnectTimeout=10 -o ConnectionAttempts=1 -o ClearAllForwardings=yes \
         -o PermitLocalCommand=no -o RemoteCommand=none -- {destination} jetd connect --stdio"
    );
    assert!(lines.iter().all(|line| *line == fixed), "{lines:#?}");
    step(
        "ssh",
        &format!("{} spawns, all with the fixed arguments", lines.len()),
    );
}
