//! Git delivery through the public protocol and disposable repositories.
mod support;

use jet_protocol::{
	RetentionPolicy, SettingKey, SettingScope, SettingSelection, SettingValue,
	WorkingTreeRequest,
};
use pretty_assertions::assert_eq;
use uuid::Uuid;

#[tokio::test]
async fn delivery_steps_resolve_independently_and_survive_restart() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join("jet");
	let mut daemon = support::start_jetd(&home).await;
	let owner = Uuid::new_v4();
	let client = support::connect(&daemon, owner).await;
	let root = support::init_repository(&dir.path().join("repo"));
	let project = client
		.register_project(Uuid::now_v7(), root.to_str().unwrap())
		.await
		.unwrap();
	let conversation = client
		.create_conversation_in(
			Uuid::now_v7(),
			RetentionPolicy::Retain,
			WorkingTreeRequest::LocalCheckout {
				project_id: project.project_id,
			},
		)
		.await
		.unwrap();
	let scope = SettingScope::Conversation {
		conversation_id: conversation.conversation_id,
	};
	let keys = [
		SettingKey::GitAutoBranch,
		SettingKey::GitAutoCommit,
		SettingKey::GitAutoPush,
		SettingKey::GitAutoDraftPullRequest,
	];
	for key in keys {
		assert_eq!(
			client
				.settings(scope, SettingSelection::Key { key })
				.await
				.unwrap()
				.settings[0]
				.value,
			SettingValue::Flag(false)
		);
		client
			.set_setting(
				Uuid::now_v7(),
				key,
				SettingScope::Project {
					project_id: project.project_id,
				},
				SettingValue::Flag(true),
			)
			.await
			.unwrap();
	}
	client
		.set_setting(
			Uuid::now_v7(),
			SettingKey::GitAutoCommit,
			scope,
			SettingValue::Flag(false),
		)
		.await
		.unwrap();
	daemon.child.kill().await.unwrap();
	let daemon = support::start_jetd(&home).await;
	let client = support::connect(&daemon, owner).await;
	for key in keys {
		assert_eq!(
			client
				.settings(scope, SettingSelection::Key { key })
				.await
				.unwrap()
				.settings[0]
				.value,
			SettingValue::Flag(key != SettingKey::GitAutoCommit)
		);
	}
	let command_id = Uuid::now_v7();
	let command = jet_protocol::CommandRequest::DeliverGit {
		conversation_id: conversation.conversation_id,
		checkpoint: None,
		operation: jet_protocol::GitOperation::Branch {
			name: "manual/from-protocol".into(),
		},
	};
	let receipt = client
		.execute_command(command_id, command.clone())
		.await
		.unwrap();
	let result =
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				let result = client
					.git_deliveries(conversation.conversation_id)
					.await
					.unwrap();
				if result.first().is_some_and(|d| {
					d.outcome != jet_protocol::GitDeliveryOutcome::Pending
				}) {
					break result;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
	assert!(
		matches!(&result[0].outcome, jet_protocol::GitDeliveryOutcome::Completed { branch: Some(name), .. } if name == "manual/from-protocol")
	);
	assert_eq!(
		client.execute_command(command_id, command).await.unwrap(),
		receipt
	);
}
