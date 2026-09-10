//! Auto-continue configuration through the public Jet protocol.
mod support;
use jet_protocol::{AutoContinuePolicy, AutoContinueTarget, CredentialSource};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn binding_policy_round_trips_and_older_clients_are_refused() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	let mut daemon = start_jetd(&home).await;
	let owner = Uuid::new_v4();
	let client = connect(&daemon, owner).await;
	let binding = client
		.bind_account(
			Uuid::now_v7(),
			"openai",
			"Work",
			None,
			CredentialSource::HarnessNative,
		)
		.await
		.unwrap();
	let target = AutoContinueTarget::AccountBinding(binding.binding_id);
	let policy = AutoContinuePolicy::Retry {
		delay_ms: 60_000,
		max_delay_ms: 600_000,
		max_retries: 3,
		message: "Continue".into(),
	};
	let command_id = Uuid::now_v7();
	client
		.set_auto_continue(command_id, target, policy.clone())
		.await
		.unwrap();
	let before = client.auto_continue(target).await.unwrap();
	assert_eq!(before.policy, policy);
	daemon.child.kill().await.unwrap();
	let daemon = start_jetd(&home).await;
	let client = connect(&daemon, owner).await;
	client
		.set_auto_continue(command_id, target, policy.clone())
		.await
		.unwrap();
	assert_eq!(client.auto_continue(target).await.unwrap().policy, policy);
	let mut hello = support::hello(owner);
	hello.minor = jet_protocol::AUTO_CONTINUE_MINOR - 1;
	let (mut connection, _) = support::handshake_raw(&daemon, &hello).await;
	for message in [
		json!({"kind":"query","id":1,"query":{"type":"auto_continue","target":target}}),
		json!({"kind":"command","id":2,"command_id":Uuid::now_v7(),"command":{"type":"set_auto_continue","target":target,"policy":{"mode":"off"}}}),
	] {
		connection.send(&message).await;
		let refused: Value = connection.receive().await;
		assert_eq!(refused["error"]["code"], "protocol.unsupported_minor");
	}
	assert_eq!(client.auto_continue(target).await.unwrap().policy, policy);
}
