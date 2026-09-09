//! Craft lifecycle behavior through real daemon, Craft, and helper processes.
#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use assertions::wait_for;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

// Each installed version reads its own immutable declaration, as a real Craft does.
fn stage(home: &Path, version: &str) -> String {
	let crafts = home.join("crafts");
	let manifest = crafts.join("fake.json");
	let frozen = crafts.join(format!("{version}.json"));
	let program = crafts.join(format!("{version}-craft"));
	let script = std::fs::read_to_string(crafts.join("fake-craft"))
		.unwrap()
		.replace(manifest.to_str().unwrap(), frozen.to_str().unwrap());
	let digest = format!("{:x}", Sha256::digest(script.as_bytes()));
	let mut declaration: Value =
		serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
	declaration["executable"] = json!(program);
	declaration["sha256"] = json!(digest);
	std::fs::write(&program, script).unwrap();
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	std::fs::write(&frozen, declaration.to_string()).unwrap();
	std::fs::write(manifest, declaration.to_string()).unwrap();
	digest
}

async fn start(
	client: &jet_client::Client,
	wire: &mut support::RawConnection,
	root: &Path,
) -> (Uuid, String) {
	let project = client
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	let conversation = client
		.create_conversation_in(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout {
				project_id: project.project_id,
			},
		)
		.await
		.unwrap();
	wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
	let admitted: Value = wire.receive().await;
	let run = admitted["result"]["run_id"].as_str().unwrap().to_owned();
	wait_for(wire, &run, "waiting_for_approval").await;
	(conversation.conversation_id, run)
}

#[tokio::test]
async fn subsequent_runs_select_the_update_while_active_runs_keep_their_digest()
{
	tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install(&home);
        stage(&home, "v1");
        let daemon = start_jetd(&home).await;
        let client_id = Uuid::new_v4();
        let client = connect(&daemon, client_id).await;
        let mut wire = connect_raw(&daemon, client_id).await;
        let root = init_repository(&dir.path().join("repo"));
        let (conversation, run) = start(&client, &mut wire, &root).await;
        stage(&home, "v2");
        std::fs::write(root.join("continue"), "go").unwrap();
        wait_for(&mut wire, &run, "completed").await;
        std::fs::remove_file(root.join("continue")).unwrap();
        wire.send(&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":{"type":"submit_turn","conversation_id":conversation,"prompt":"Make a change"}})).await;
        assert_eq!(wire.receive::<Value>().await["kind"], "command_result");
        loop {
            let runs = client.conversation(conversation).await.unwrap().runs;
            if let Some(next) = runs.iter().find(|next| next.run_id.to_string() != run) {
                wait_for(&mut wire, &next.run_id.to_string(), "waiting_for_approval").await;
                assert!(home.join("crafts/v2.starts").exists(), "the subsequent Run must use the staged digest");
                std::fs::write(root.join("continue"), "go").unwrap();
                wait_for(&mut wire, &next.run_id.to_string(), "completed").await;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn disabling_blocks_new_runs_and_force_preserves_the_live_helper() {
	tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install(&home);
        stage(&home, "v1");
        let mut daemon = start_jetd(&home).await;
        let client_id = Uuid::new_v4();
        let client = connect(&daemon, client_id).await;
        let mut wire = connect_raw(&daemon, client_id).await;
        let root = init_repository(&dir.path().join("repo"));
        let (conversation, run) = start(&client, &mut wire, &root).await;
        let before = wait_for(&mut wire, &run, "waiting_for_approval").await;
        for mode in ["wait", "force"] {
            wire.send(&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":{"type":"disable_craft","craft_id":"fake","mode":mode}})).await;
            let reply = wire.receive::<Value>().await;
            assert_eq!(reply["kind"], "command_result", "{reply}");
            if mode == "wait" {
                assert_eq!(wait_for(&mut wire, &run, "waiting_for_approval").await["processes"], before["processes"]);
            }
            wire.send(&json!({"kind":"command","id":4,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation,"craft":"fake","prompt":"Make a change"}})).await;
            let refusal = wire.receive::<Value>().await;
            assert_eq!(refusal["error"]["code"], "craft.disabled", "{refusal}");
        }
        daemon.child.kill().await.unwrap();
        let daemon = start_jetd(&home).await;
        let mut wire = connect_raw(&daemon, client_id).await;
        wire.send(&json!({"kind":"query","id":5,"query":{"type":"run_execution","run_id":run}})).await;
        let after = wire.receive::<Value>().await;
        assert_eq!(after["result"]["needs_attention"], true);
        assert_eq!(after["result"]["processes"], before["processes"]);
        assert_eq!(std::fs::read_to_string(home.join("crafts/v1.starts")).unwrap().lines().count(), 1);
        // The Harness still makes progress while the Craft remains disabled.
        std::fs::write(root.join("continue"), "go").unwrap();
        loop {
            if root.join("harness-finished").exists() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
}

async fn craft_streams(home: &Path) -> Vec<tokio::net::UnixStream> {
	let mut streams = Vec::new();
	for entry in std::fs::read_dir(home.join("runtime")).unwrap() {
		let path = entry.unwrap().path();
		if path
			.file_name()
			.unwrap()
			.to_string_lossy()
			.starts_with("c-")
			&& path
				.extension()
				.is_some_and(|extension| extension == "sock")
		{
			streams.push(tokio::net::UnixStream::connect(path).await.unwrap());
		}
	}
	streams
}

async fn assert_crafts_stopped(streams: Vec<tokio::net::UnixStream>) {
	use tokio::io::AsyncReadExt;
	for mut stream in streams {
		assert_eq!(
			tokio::time::timeout(Duration::from_secs(5), stream.read(&mut [0]))
				.await
				.unwrap()
				.unwrap(),
			0
		);
	}
}

#[tokio::test]
async fn force_disable_stops_idle_versions_too() {
	tokio::time::timeout(Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		for version in ["v1", "v2"] {
			stage(&home, version);
			let root = init_repository(&dir.path().join(version));
			let (_, run) = start(&client, &mut wire, &root).await;
			std::fs::write(root.join("continue"), "go").unwrap();
			wait_for(&mut wire, &run, "completed").await;
		}
		let streams = craft_streams(&home).await;
		assert_eq!(streams.len(), 2);
		client
			.disable_craft(
				Uuid::now_v7(),
				"fake".into(),
				jet_protocol::CraftDisableMode::Force,
			)
			.await
			.unwrap();
		assert_crafts_stopped(streams).await;
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn daemon_crash_retires_its_crafts_before_recovering_the_same_helpers() {
	tokio::time::timeout(Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		stage(&home, "v1");
		let mut command = support::jetd(Path::new("jet"));
		command.current_dir(dir.path());
		let mut daemon = support::start_jetd_process(&mut command).await;
		daemon.socket = dir.path().join(&daemon.socket);
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let (_, run) = start(&client, &mut wire, &root).await;
		let before = wait_for(&mut wire, &run, "waiting_for_approval").await;
		let streams = craft_streams(&home).await;
		assert_eq!(streams.len(), 1);
		daemon.child.kill().await.unwrap();
		let daemon = start_jetd(&home).await;
		assert_crafts_stopped(streams).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		assert_eq!(
			wait_for(&mut wire, &run, "waiting_for_approval").await["processes"],
			before["processes"]
		);
		assert_eq!(
			std::fs::read_to_string(home.join("crafts/v1.starts"))
				.unwrap()
				.lines()
				.count(),
			2
		);
		std::fs::write(root.join("continue"), "go").unwrap();
		wait_for(&mut wire, &run, "completed").await;
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn only_signed_revocations_stop_a_digest_and_the_barrier_survives_restart()
 {
	tokio::time::timeout(Duration::from_secs(30), async {
        use ed25519_dalek::{Signer, SigningKey};
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let home = dir.path().join("jet");
        fixture::install(&home);
        let digest = stage(&home, "v1");
        let signer = SigningKey::from_bytes(&[41; 32]);
        let key = dir.path().join("release-key");
        std::fs::write(&key, signer.verifying_key().as_bytes()).unwrap();
        let mut daemon = support::start_jetd_process(support::jetd(&home).arg("--release-verification-key").arg(&key)).await;
        let client_id = Uuid::new_v4();
        let client = connect(&daemon, client_id).await;
        let mut wire = connect_raw(&daemon, client_id).await;
        let root = init_repository(&dir.path().join("repo"));
        let (conversation, run) = start(&client, &mut wire, &root).await;
        let mut hello = support::hello(client_id);
        hello.minor = jet_protocol::CRAFT_LIFECYCLE_MINOR - 1;
        let (mut legacy, _) = support::handshake_raw(&daemon, &hello).await;
        let metadata = home.join("crafts/revocations.json");
        let invalid = json!({"digests":[digest],"signature":vec![0_u8;64]});
        std::fs::write(&metadata, invalid.to_string()).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        wait_for(&mut wire, &run, "waiting_for_approval").await;
        assert!(client.security_audit_after(0).await.unwrap().entries.iter().all(|entry| entry.decision != "craft.digest_revoked"));
        let message = format!("jet.craft-revocations.v1\0{digest}\n");
        let signed = json!({"digests":[digest],"signature":signer.sign(message.as_bytes()).to_bytes().to_vec()});
        std::fs::write(&metadata, signed.to_string()).unwrap();
        loop {
            wire.send(&json!({"kind":"query","id":5,"query":{"type":"run_execution","run_id":run}})).await;
            if wire.receive::<Value>().await["result"]["needs_attention"] == true { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let audit = client.security_audit_after(0).await.unwrap();
        let revocations: Vec<_> = audit.entries.into_iter().filter(|entry| entry.decision == "craft.digest_revoked").collect();
        assert_eq!(revocations.len(), 1);
        assert_eq!((&revocations[0].actor, revocations[0].target.kind.as_str(), revocations[0].target.identity.as_deref(), revocations[0].outcome), (&jet_protocol::AuditActor::CraftRevocation, "craft_digest", Some(digest.as_str()), jet_protocol::AuditOutcome::Succeeded));
        legacy.send(&json!({"kind":"query","id":9,"query":{"type":"security_audit","after":"0"}})).await;
        assert_eq!(legacy.receive::<Value>().await["error"]["code"], "audit.actor_incompatible");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(client.security_audit_after(0).await.unwrap().entries.into_iter().filter(|entry| entry.decision == "craft.digest_revoked").collect::<Vec<_>>(), revocations);
        daemon.child.kill().await.unwrap();
        std::fs::remove_file(metadata).unwrap();
        let daemon = start_jetd(&home).await;
        let client = connect(&daemon, client_id).await;
        assert_eq!(client.security_audit_after(0).await.unwrap().entries.into_iter().filter(|entry| entry.decision == "craft.digest_revoked").collect::<Vec<_>>(), revocations);
        let mut wire = connect_raw(&daemon, client_id).await;
        wire.send(&json!({"kind":"command","id":6,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation,"craft":"fake","prompt":"Make a change"}})).await;
        let refusal = wire.receive::<Value>().await;
        assert_eq!(refusal["error"]["code"], "craft.revoked", "{refusal}");
        assert_eq!(std::fs::read_to_string(home.join("crafts/v1.starts")).unwrap().lines().count(), 1);
        std::fs::write(root.join("continue"), "go").unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn active_digests_multiplex_runs_and_restart_without_switching_versions()
{
	tokio::time::timeout(Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		stage(&home, "v1");
		let daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		let mut runs = Vec::new();
		for name in ["one", "two", "three"] {
			if name == "three" {
				stage(&home, "v2");
			}
			let root = init_repository(&dir.path().join(name));
			let (_, run) = start(&client, &mut wire, &root).await;
			let before =
				wait_for(&mut wire, &run, "waiting_for_approval").await;
			runs.push((root, run, before));
		}
		let starts = |version: &str| {
			std::fs::read_to_string(
				home.join(format!("crafts/{version}.starts")),
			)
			.unwrap()
		};
		assert_eq!(
			(starts("v1").lines().count(), starts("v2").lines().count()),
			(1, 1)
		);
		let pid = starts("v1").trim().parse::<i32>().unwrap();
		rustix::process::kill_process(
			rustix::process::Pid::from_raw(pid).unwrap(),
			rustix::process::Signal::KILL,
		)
		.unwrap();
		wait_for(&mut wire, &runs[0].1, "reconnecting").await;
		for (_, run, before) in &runs {
			assert_eq!(
				wait_for(&mut wire, run, "waiting_for_approval").await["processes"],
				before["processes"]
			);
		}
		assert_eq!(
			(starts("v1").lines().count(), starts("v2").lines().count()),
			(2, 1)
		);
		for (root, run, _) in runs {
			std::fs::write(root.join("continue"), "go").unwrap();
			wait_for(&mut wire, &run, "completed").await;
		}
	})
	.await
	.unwrap();
}
