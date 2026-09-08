//! Visa admission and execution through the public Jet protocol.
#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "visa/failure_tests.rs"]
mod failure_tests;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

struct Identity(Uuid, ed25519_dalek::SigningKey);
impl jet_client::ClientIdentity for Identity {
	fn client_id(&self) -> Uuid {
		self.0
	}
	async fn sign(&self, transcript: &[u8]) -> std::io::Result<[u8; 64]> {
		use ed25519_dalek::Signer;
		Ok(self.1.sign(transcript).to_bytes())
	}
}

async fn remote(
	home: &std::path::Path,
	identity: &Identity,
) -> (tokio::process::Child, jet_client::Client) {
	let mut bridge = tokio::process::Command::new(env!("CARGO_BIN_EXE_jetd"))
		.args(["connect", "--stdio", "--home"])
		.arg(home)
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let client = jet_client::Client::connect_remote(
		bridge.stdout.take().unwrap(),
		bridge.stdin.take().unwrap(),
		identity,
	)
	.await
	.unwrap();
	(bridge, client)
}

async fn selection(
	client: &jet_client::Client,
	root: &std::path::Path,
) -> jet_protocol::VisaRunRequest {
	let project = client
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	let conversation = client
		.create_conversation_in(
			Uuid::now_v7(),
			jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::Workspace {
				project_id: project.project_id,
				base: jet_protocol::BaseSelection::Head,
				seed: jet_protocol::SeedSelection::None,
			},
		)
		.await
		.unwrap();
	jet_protocol::VisaRunRequest {
		conversation_id: conversation.conversation_id,
		destination_plane_id: client.status().await.unwrap().plane_id,
		account_binding_id: Uuid::now_v7(),
		craft: "fake".into(),
		prompt: "Make a change".into(),
	}
}

fn install(home: &std::path::Path) {
	fixture::install(home);
	let path = home.join("crafts/fake.json");
	let mut manifest: Value =
		serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
	manifest["specification"]["harness"] = json!("codex");
	std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

async fn native_binding(
	client: &jet_client::Client,
) -> jet_protocol::AccountBinding {
	client
		.bind_account(
			Uuid::now_v7(),
			"openai",
			"Destination",
			None,
			jet_protocol::CredentialSource::HarnessNative,
		)
		.await
		.unwrap()
}

#[tokio::test]
async fn visa_never_substitutes_native_auth_for_another_binding() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	install(&home);
	let daemon = support::start_jetd_with_credential_store(&home).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let mut request =
		selection(&client, &init_repository(&dir.path().join("repo"))).await;
	request.prompt = "Fail after spawn".into();
	for (provider, source, code) in [
		(
			"anthropic",
			jet_protocol::CredentialSource::HarnessNative,
			"visa.provider_mismatch",
		),
		(
			"openai",
			jet_protocol::CredentialSource::ExternalHelper {
				helper: "test-credentials".into(),
			},
			"visa.credential_source_unsupported",
		),
		(
			"openai",
			jet_protocol::CredentialSource::PlatformStore,
			"visa.credential_source_unsupported",
		),
		(
			"openai",
			jet_protocol::CredentialSource::SessionOnly,
			"visa.credential_source_unsupported",
		),
	] {
		let binding = client
			.bind_account(Uuid::now_v7(), provider, "Destination", None, source)
			.await
			.unwrap();
		request.account_binding_id = binding.binding_id;
		let error = client
			.start_visa_run(Uuid::now_v7(), request.clone())
			.await
			.unwrap_err();
		let jet_client::ClientError::Remote(error) = error else {
			panic!("{error:?}")
		};
		assert_eq!(error.code, code);
	}
	assert!(
		client
			.conversation(request.conversation_id)
			.await
			.unwrap()
			.runs
			.is_empty()
	);
}

#[tokio::test]
async fn queued_visa_work_cannot_outlive_its_binding() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		install(&home);
		let daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let mut request =
			selection(&client, &init_repository(&dir.path().join("repo")))
				.await;
		let binding = native_binding(&client).await;
		request.account_binding_id = binding.binding_id;
		let workspace = client
			.conversation(request.conversation_id)
			.await
			.unwrap()
			.workspace
			.unwrap();
		std::fs::write(
			std::path::Path::new(&workspace.root).join("continue"),
			"finish",
		)
		.unwrap();
		let run = client
			.start_visa_run(Uuid::now_v7(), request.clone())
			.await
			.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		assertions::wait_for(&mut wire, &run.run_id.to_string(), "completed")
			.await;
		client
			.unbind_account(Uuid::now_v7(), binding.binding_id)
			.await
			.unwrap();
		client
			.execute_command(
				Uuid::now_v7(),
				jet_protocol::CommandRequest::SubmitTurn {
					conversation_id: request.conversation_id,
					source: jet_protocol::TurnSource::User,
					prompt: "Make a change".into(),
				},
			)
			.await
			.unwrap();
		// Completing independent work proves that the shared execution worker
		// made its queued-admission pass; no timing-only negative assertion.
		let mut other =
			selection(&client, &init_repository(&dir.path().join("other")))
				.await;
		other.account_binding_id = native_binding(&client).await.binding_id;
		other.prompt = "Fail after spawn".into();
		let other_run =
			client.start_visa_run(Uuid::now_v7(), other).await.unwrap();
		assertions::wait_for(
			&mut wire,
			&other_run.run_id.to_string(),
			"failed",
		)
		.await;
		let snapshot =
			client.conversation(request.conversation_id).await.unwrap();
		assert_eq!(snapshot.runs.len(), 1);
		let queue = client.turn_queue(request.conversation_id).await.unwrap();
		assert_eq!(
			queue
				.turns
				.iter()
				.map(|turn| turn.state)
				.collect::<Vec<_>>(),
			vec![jet_protocol::TurnState::Queued]
		);
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn visa_requires_a_binding_on_the_destination() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	install(&home);
	let daemon = start_jetd(&home).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let request =
		selection(&client, &init_repository(&dir.path().join("repo"))).await;
	let error = client
		.start_visa_run(Uuid::now_v7(), request.clone())
		.await
		.unwrap_err();
	let jet_client::ClientError::Remote(error) = error else {
		panic!("{error:?}")
	};
	assert_eq!(error.code, "visa.binding_unavailable");
	assert!(
		client
			.conversation(request.conversation_id)
			.await
			.unwrap()
			.runs
			.is_empty()
	);
}

#[tokio::test]
async fn remote_visa_keeps_destination_authority_through_auth_disconnect_and_restart()
 {
	tokio::time::timeout(std::time::Duration::from_secs(40), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let origin = start_jetd(&dir.path().join("origin")).await;
		let origin_client = connect(&origin, Uuid::new_v4()).await;
		let home = dir.path().join("destination");
		install(&home);
		let mut daemon = start_jetd(&home).await;
		let identity = Identity(Uuid::new_v4(), ed25519_dalek::SigningKey::from_bytes(&[41; 32]));
		let owner = connect(&daemon, Uuid::new_v4()).await;
		let local = connect(&daemon, identity.0).await;
		support::pairing::pair(&owner, &local, identity.0, &identity.1).await;
		let (mut bridge, client) = remote(&home, &identity).await;
		let root = init_repository(&dir.path().join("repo"));
		let mut request = selection(&client, &root).await;
		let binding = native_binding(&client).await;
		request.account_binding_id = binding.binding_id;
		let workspace = client.conversation(request.conversation_id).await.unwrap().workspace.unwrap();
		let workspace_root = std::path::Path::new(&workspace.root);
		std::fs::write(workspace_root.join("auth-wait"), "await native login").unwrap();
		let command_id = Uuid::now_v7();
		let admitted = client.start_visa_run(command_id, request.clone()).await.unwrap();
		let mut wire = connect_raw(&daemon, identity.0).await;
		let waiting = assertions::wait_for(&mut wire, &admitted.run_id.to_string(), "waiting_for_auth").await;
		bridge.kill().await.unwrap();
		assert!(client.status().await.is_err());
		assert_eq!(assertions::wait_for(&mut wire, &admitted.run_id.to_string(), "waiting_for_auth").await, waiting);
		daemon.child.kill().await.unwrap();
		let daemon = start_jetd(&home).await;
		let (_bridge, client) = remote(&home, &identity).await;
		assert_eq!(client.start_visa_run(command_id, request.clone()).await.unwrap(), admitted);
		let mut wire = connect_raw(&daemon, identity.0).await;
		let recovered = assertions::wait_for(&mut wire, &admitted.run_id.to_string(), "waiting_for_auth").await;
		assert_eq!(recovered["processes"], waiting["processes"]);
		std::fs::write(workspace_root.join("authenticated"), "native authentication completed").unwrap();
		std::fs::write(workspace_root.join("continue"), "finish").unwrap();
		let completed = assertions::wait_for(&mut wire, &admitted.run_id.to_string(), "completed").await;
		assert_eq!(completed["visa"], json!({"plane_id":request.destination_plane_id,"account_binding_id":binding.binding_id}));
		assert_eq!(completed["exit_code"], 0);
		assert_eq!(client.conversation(request.conversation_id).await.unwrap().runs.len(), 1);
		assert!(origin_client.conversations().await.unwrap().conversations.is_empty());
		assert!(origin_client.account_bindings(jet_protocol::CapabilityObservation::Fresh).await.unwrap().bindings.is_empty());
		assert_eq!(std::fs::read_to_string(workspace_root.join("result.txt")).unwrap(), "Harness work\n");
		assert!(!root.join("result.txt").exists());
		let diff = client.change_diff(admitted.run_id, jet_protocol::DiffScope::Turn { turn: 1 }).await.unwrap();
		assert_eq!(diff.workspace_id, Some(workspace.workspace_id));
		assert!(diff.patch.contains("+Harness work\n"));
		let journal = assertions::all_events(&client).await;
		assert!(journal.events.iter().any(|event| event.run_id == Some(admitted.run_id)
			&& event.actor == jet_protocol::Actor::InteractiveClient { client_id: identity.0 }));
		let mut hello = support::hello(identity.0);
		hello.minor = jet_protocol::VISA_RUNS_MINOR - 1;
		let (mut older, _) = support::handshake_raw(&daemon, &hello).await;
		older.send(&json!({"kind":"query","id":2,"query":{"type":"run_execution","run_id":admitted.run_id}})).await;
		assert!(older.receive::<Value>().await["result"].get("visa").is_none());
		older.send(&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":jet_protocol::CommandRequest::StartVisaRun(request)})).await;
		assert_eq!(older.receive::<Value>().await["error"]["code"], "protocol.unsupported_minor");
	}).await.unwrap();
}

#[tokio::test]
async fn visa_refuses_a_destination_other_than_the_home_plane() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let daemon = start_jetd(&dir.path().join("jet")).await;
	let client_id = Uuid::new_v4();
	let client = connect(&daemon, client_id).await;
	let mut wire = connect_raw(&daemon, client_id).await;
	wire.send(
		&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{
			"type":"start_visa_run", "conversation_id":Uuid::now_v7(),
			"destination_plane_id":Uuid::now_v7(), "account_binding_id":Uuid::now_v7(),
			"craft":"fake", "prompt":"Make a change"
		}}),
	)
	.await;
	let refused: Value = wire.receive().await;
	assert_eq!(refused["error"]["code"], "visa.destination_mismatch");
	assert!(
		client
			.conversations()
			.await
			.unwrap()
			.conversations
			.is_empty()
	);
}
