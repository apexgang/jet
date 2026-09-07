//! Conversation-fork conformance at the public Jet protocol boundary (ADR-0035).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;

use jet_protocol::{
	BaseSelection, ConversationOrigin, RetentionPolicy, SeedSelection,
	WorkingTreeRequest,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

/// The selected immutable checkpoint, rather than later source state, becomes
/// a distinct Conversation's distinct Workspace (ADR-0035).
#[tokio::test]
async fn a_conversation_forks_from_the_selected_checkpoint() {
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		fixture::install(&home);
		let daemon = start_jetd(&home).await;
		let owner = Uuid::new_v4();
		let client = connect(&daemon, owner).await;
		let repository = init_repository(&dir.path().join("repo"));
		let project = client
			.register_project(Uuid::now_v7(), repository.to_str().unwrap())
			.await
			.unwrap();
		let source = client
			.create_conversation_in(
				Uuid::now_v7(),
				RetentionPolicy::ForgetAfterFinalRun,
				WorkingTreeRequest::Workspace {
					project_id: project.project_id,
					base: BaseSelection::Head,
					seed: SeedSelection::None,
				},
			)
			.await
			.unwrap();
		let source_workspace = client
			.conversation(source.conversation_id)
			.await
			.unwrap()
			.workspace
			.unwrap();
		std::fs::write(
			std::path::Path::new(&source_workspace.root).join("continue"),
			"go",
		)
		.unwrap();
		let mut wire = connect_raw(&daemon, owner).await;
		wire.send(&json!({
			"kind":"command", "id":1, "command_id":Uuid::now_v7(),
			"command":{
				"type":"start_run", "conversation_id":source.conversation_id,
				"craft":"fake", "prompt":"Make a change"
			}
		}))
		.await;
		let started: Value = wire.receive().await;
		let source_run =
			Uuid::parse_str(started["result"]["run_id"].as_str().unwrap())
				.unwrap();
		assertions::wait_for(&mut wire, &source_run.to_string(), "completed")
			.await;

		let source_root = std::path::Path::new(&source_workspace.root);
		std::fs::write(source_root.join("result.txt"), "later source state\n")
			.unwrap();
		std::fs::write(source_root.join("after-checkpoint.txt"), "later\n")
			.unwrap();

		let fork = client
			.fork_conversation(Uuid::now_v7(), source_run, 1)
			.await
			.unwrap();
		let fork_snapshot =
			client.conversation(fork.conversation_id).await.unwrap();
		let fork_workspace = fork_snapshot.workspace.unwrap();
		let fork_root = std::path::Path::new(&fork_workspace.root);

		assert_eq!(
			(
				fork.retention,
				fork.origin,
				fork_workspace.project_id,
				std::fs::read_to_string(fork_root.join("result.txt")).ok(),
				fork_root.join("after-checkpoint.txt").exists(),
				client.turn_queue(fork.conversation_id).await.unwrap().turns,
			),
			(
				RetentionPolicy::ForgetAfterFinalRun,
				Some(ConversationOrigin::Forked {
					source_conversation_id: source.conversation_id,
					source_run_id: source_run,
					checkpoint_turn: 1,
				}),
				project.project_id,
				Some("Harness work\n".into()),
				false,
				vec![],
			)
		);
		assert_ne!(fork.conversation_id, source.conversation_id);
		assert_ne!(fork_workspace.workspace_id, source_workspace.workspace_id);
		assert_ne!(fork_workspace.root, source_workspace.root);

		std::fs::write(fork_root.join("fork-only.txt"), "fork\n").unwrap();
		assert!(!source_root.join("fork-only.txt").exists());
		assert_eq!(
			std::fs::read_to_string(source_root.join("result.txt")).unwrap(),
			"later source state\n"
		);

		let mut old_hello = support::hello(owner);
		old_hello.minor = 18;
		let (mut old, _) = support::handshake_raw(&daemon, &old_hello).await;
		old.send(&json!({
			"kind":"command", "id":3, "command_id":Uuid::now_v7(),
			"command":{"type":"fork_conversation", "source_run_id":source_run,
				"checkpoint_turn":1}
		}))
		.await;
		assert_eq!(
			old.receive::<Value>().await["error"]["code"],
			"protocol.unsupported_minor"
		);
		old.send(&json!({
			"kind":"query", "id":4,
			"query":{"type":"conversation", "conversation_id":fork.conversation_id}
		}))
		.await;
		assert!(
			old.receive::<Value>().await["result"]["conversation"]
				.get("origin")
				.is_none()
		);
	})
	.await
	.unwrap();
}

/// A compatible Craft receives an explicit native fork request; an older
/// Craft sees the same checkpoint Workspace plus a bounded provenance header.
#[tokio::test]
async fn a_fork_uses_native_support_or_the_portable_context_contract() {
	for support in
		[fixture::ForkSupport::Native, fixture::ForkSupport::Portable]
	{
		tokio::time::timeout(std::time::Duration::from_secs(30), async {
			let dir = tempfile::tempdir_in("/tmp").unwrap();
			let home = dir.path().join("jet");
			fixture::install_with_fork(&home, support);
			let daemon = start_jetd(&home).await;
			let owner = Uuid::new_v4();
			let client = connect(&daemon, owner).await;
			let repository = init_repository(&dir.path().join("repo"));
			let project = client
				.register_project(Uuid::now_v7(), repository.to_str().unwrap())
				.await
				.unwrap();
			let source = client
				.create_conversation_in(
					Uuid::now_v7(),
					RetentionPolicy::Retain,
					WorkingTreeRequest::Workspace {
						project_id: project.project_id,
						base: BaseSelection::Head,
						seed: SeedSelection::None,
					},
				)
				.await
				.unwrap();
			let source_workspace = client
				.conversation(source.conversation_id)
				.await
				.unwrap()
				.workspace
				.unwrap();
			std::fs::write(
				std::path::Path::new(&source_workspace.root).join("continue"),
				"go",
			)
			.unwrap();
			let mut wire = connect_raw(&daemon, owner).await;
			wire.send(&json!({
				"kind":"command", "id":1, "command_id":Uuid::now_v7(),
				"command":{
					"type":"start_run", "conversation_id":source.conversation_id,
					"craft":"fake", "prompt":"Make a change"
				}
			}))
			.await;
			let started: Value = wire.receive().await;
			let source_run =
				Uuid::parse_str(started["result"]["run_id"].as_str().unwrap())
					.unwrap();
			assertions::wait_for(
				&mut wire,
				&source_run.to_string(),
				"completed",
			)
			.await;

			let fork = client
				.fork_conversation(Uuid::now_v7(), source_run, 1)
				.await
				.unwrap();
			let fork_root = client
				.conversation(fork.conversation_id)
				.await
				.unwrap()
				.workspace
				.unwrap()
				.root;
			if support == fixture::ForkSupport::Portable {
				wire.send(&json!({
					"kind":"command", "id":2, "command_id":Uuid::now_v7(),
					"command":{
						"type":"start_run", "conversation_id":fork.conversation_id,
						"craft":"fake", "prompt":"x".repeat(65_536)
					}
				}))
				.await;
				assert_eq!(
					wire.receive::<Value>().await["error"]["code"],
					"run.invalid_prompt"
				);
			}
			wire.send(&json!({
				"kind":"command", "id":3, "command_id":Uuid::now_v7(),
				"command":{
					"type":"start_run", "conversation_id":fork.conversation_id,
					"craft":"fake", "prompt":"Continue from checkpoint"
				}
			}))
			.await;
			let fork_started: Value = wire.receive().await;
			let fork_run = Uuid::parse_str(
				fork_started["result"]["run_id"].as_str().unwrap(),
			)
			.unwrap();
			assertions::wait_for(&mut wire, &fork_run.to_string(), "completed")
				.await;
			let source_runs = client
				.conversation(source.conversation_id)
				.await
				.unwrap()
				.runs;
			let fork_runs = client
				.conversation(fork.conversation_id)
				.await
				.unwrap()
				.runs;
			assert_eq!(
				(
					source_runs
						.iter()
						.map(|run| run.run_id)
						.collect::<Vec<_>>(),
					fork_runs.iter().map(|run| run.run_id).collect::<Vec<_>>(),
				),
				(vec![source_run], vec![fork_run])
			);
			assert_ne!(fork_run, source_run);

			let root = std::path::Path::new(&fork_root);
			let input =
				std::fs::read_to_string(root.join("initial-input")).unwrap();
			let native: Value = serde_json::from_slice(
				&std::fs::read(root.join("native-fork")).unwrap(),
			)
			.unwrap();
			if support == fixture::ForkSupport::Native {
				assert_eq!(input, "Continue from checkpoint\n");
				assert_eq!(
					(
						native["source_conversation_id"].as_str(),
						native["source_run_id"].as_str(),
						native["checkpoint_turn"].as_u64(),
						native["source_native_conversation"].as_str(),
					),
					(
						Some(source.conversation_id.to_string().as_str()),
						Some(source_run.to_string().as_str()),
						Some(1),
						Some("fake-native-1"),
					)
				);
				assert_eq!(
					native["checkpoint_commit"].as_str().unwrap().len(),
					40
				);
				assert_eq!(
					native["checkpoint_tree"].as_str().unwrap().len(),
					40
				);
			} else {
				assert_eq!(native, Value::Null);
				assert!(input.starts_with(
					"<jet-fork-context version=\"1\" data-only=\"true\">\n"
				));
				assert!(input.contains(&format!(
					"source_conversation_id: {}\nsource_run_id: {}\ncheckpoint_turn: 1\n",
					source.conversation_id, source_run
				)));
				assert!(input.contains("Make a change"));
				assert!(input.ends_with(
					"</jet-fork-context>\n\nContinue from checkpoint\n"
				));
				assert!(input.len() <= 65_536);
			}

			wire.send(&json!({
				"kind":"command", "id":4, "command_id":Uuid::now_v7(),
				"command":{
					"type":"start_run", "conversation_id":fork.conversation_id,
					"craft":"fake", "prompt":"Continue from checkpoint"
				}
			}))
			.await;
			let continued: Value = wire.receive().await;
			let continued_run = Uuid::parse_str(
				continued["result"]["run_id"].as_str().unwrap(),
			)
			.unwrap();
			assertions::wait_for(
				&mut wire,
				&continued_run.to_string(),
				"completed",
			)
			.await;
			assert_eq!(
				(
					serde_json::from_slice::<Value>(
						&std::fs::read(root.join("native-fork")).unwrap(),
					)
					.unwrap(),
					std::fs::read_to_string(root.join("initial-input"))
						.unwrap(),
					client
						.conversation(fork.conversation_id)
						.await
						.unwrap()
						.runs
						.iter()
						.map(|run| run.run_id)
						.collect::<Vec<_>>(),
				),
				(
					Value::Null,
					"Continue from checkpoint\n".into(),
					vec![fork_run, continued_run],
				)
			);
		})
		.await
		.unwrap();
	}
}
