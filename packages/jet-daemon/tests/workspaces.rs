//! Black-box tests for Conversation Workspaces and Local checkouts at the
//! public Jet protocol boundary (ADR-0025).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	BaseSelection, ClientMessage, CommandRequest, CommandResponse,
	ErrorCategory, PROJECTS_MINOR, RetentionPolicy, SeedSelection, ServerHello,
	ServerMessage, WORKSPACES_MINOR, WorkingTree, WorkingTreeRequest as In,
	Workspace, WorkspaceBase, WorkspaceSeed,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, init_repository, start_jetd};
use uuid::Uuid;

/// A managed Conversation receives a Workspace at a deterministic root
/// under the Jet home, checked out from the Project detached at its base,
/// and the snapshot names both the Project and the base (ADR-0025).
#[tokio::test]
async fn a_managed_conversation_receives_a_workspace_under_the_jet_home() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();

	let conversation = client
		.create_conversation_in(
			Uuid::now_v7(),
			RetentionPolicy::Retain,
			In::Workspace {
				project_id: project.project_id,
				base: BaseSelection::Head,
				seed: SeedSelection::None,
			},
		)
		.await
		.unwrap();
	let snapshot = client
		.conversation(conversation.conversation_id)
		.await
		.unwrap();

	let root = home
		.join("workspaces")
		.join(conversation.conversation_id.to_string());
	let workspace = snapshot.workspace.clone().unwrap();
	assert_eq!(
		(
			conversation.working_tree,
			&workspace,
			root.join(".git").is_file(),
			root.is_dir(),
		),
		(
			Some(WorkingTree::Workspace {
				project_id: project.project_id,
			}),
			&Workspace {
				workspace_id: workspace.workspace_id,
				conversation_id: conversation.conversation_id,
				project_id: project.project_id,
				root: root.to_str().unwrap().into(),
				base: WorkspaceBase {
					selection: BaseSelection::Head,
					commit: workspace.base.commit.clone(),
				},
				seed: None,
				promotion: None,
				created_at_unix_ms: conversation.created_at_unix_ms,
			},
			true,
			true,
		)
	);
}

/// A Local checkout admits one live managed Run, and the refusal of a
/// second says what Jet cannot lock (ADR-0025).
#[tokio::test]
async fn a_local_checkout_refuses_a_second_live_managed_run() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();
	let mut conversations = Vec::new();
	for _ in 0..2 {
		conversations.push(
			client
				.create_conversation_in(
					Uuid::now_v7(),
					RetentionPolicy::Retain,
					In::LocalCheckout {
						project_id: project.project_id,
					},
				)
				.await
				.unwrap(),
		);
	}

	let first = client
		.create_run(Uuid::now_v7(), conversations[0].conversation_id)
		.await
		.unwrap();
	let second = client
		.create_run(Uuid::now_v7(), conversations[1].conversation_id)
		.await
		.unwrap_err();
	let ClientError::Remote(refusal) = second else {
		panic!("expected a stable remote error, got {second:?}");
	};

	assert_eq!(
		(
			first.conversation_id,
			conversations[1].working_tree,
			refusal.category,
			refusal.code.as_str(),
			refusal.message.contains("outside its management"),
		),
		(
			conversations[0].conversation_id,
			Some(WorkingTree::LocalCheckout {
				project_id: project.project_id,
			}),
			ErrorCategory::Conflict,
			"run.local_checkout_busy",
			true,
		)
	);
}

/// A client that negotiated a minor without working trees is refused one
/// with a stable error, and is told nothing of working trees on the
/// Conversations it does create (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_workspace_minor_is_refused_a_working_tree() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = PROJECTS_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == PROJECTS_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Command {
			id: 1,
			command_id: Uuid::now_v7(),
			command: CommandRequest::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: In::LocalCheckout {
					project_id: Uuid::nil(),
				},
			},
		})
		.await;
	let ServerMessage::Error { id, error } = connection.receive().await else {
		panic!("expected a refusal");
	};
	connection
		.send(&ClientMessage::Command {
			id: 2,
			command_id: Uuid::now_v7(),
			command: CommandRequest::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: In::NoProject,
			},
		})
		.await;
	let ServerMessage::CommandResult {
		id: 2,
		result: CommandResponse::ConversationCreated(created),
	} = connection.receive().await
	else {
		panic!("expected a Conversation");
	};

	assert_eq!(
		(
			id,
			error.category,
			error.code.as_str(),
			error.message.as_str(),
			created.working_tree,
		),
		(
			Some(1),
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			"a Conversation with a working tree needs protocol minor 9",
			None,
		)
	);
}

/// A Workspace seeded with the Local checkout's eligible changes holds
/// them, its snapshot says what it was seeded with, and the checkout is
/// left as it was (ADR-0025).
#[tokio::test]
async fn a_workspace_is_seeded_with_the_selected_checkout_changes() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	std::fs::write(repository.join("notes.txt"), "draft\n").unwrap();
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();

	let conversation = client
		.create_conversation_in(
			Uuid::now_v7(),
			RetentionPolicy::Retain,
			In::Workspace {
				project_id: project.project_id,
				base: BaseSelection::Head,
				seed: SeedSelection::AllEligible,
			},
		)
		.await
		.unwrap();
	let workspace = client
		.conversation(conversation.conversation_id)
		.await
		.unwrap()
		.workspace
		.unwrap();

	let root = std::path::PathBuf::from(&workspace.root);
	let seed = workspace.seed.clone().unwrap();
	assert_eq!(
		(
			std::fs::read_to_string(root.join("notes.txt")).ok(),
			&seed,
			std::fs::read_to_string(repository.join("notes.txt")).ok(),
		),
		(
			Some("draft\n".into()),
			&WorkspaceSeed {
				tree: seed.tree.clone(),
				changed_paths: 1,
			},
			Some("draft\n".into()),
		)
	);
}

/// A seed whose checkout is not at the selected base, or whose path is not
/// one the Plane accepts, is refused with a stable error and creates
/// nothing; and a client that negotiated a minor without seeds is refused
/// one (ADR-0019, ADR-0025, ADR-0101).
#[tokio::test]
async fn a_seed_the_plane_cannot_take_is_refused_with_a_stable_error() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();
	let mut refusals = Vec::new();
	for (base, seed) in [
		(
			BaseSelection::Revision {
				revision: "HEAD~0".into(),
			},
			SeedSelection::Paths {
				paths: vec!["../escape".into()],
			},
		),
		(
			BaseSelection::Revision {
				revision: "HEAD".into(),
			},
			SeedSelection::Paths {
				paths: vec!["nothere.txt".into()],
			},
		),
	] {
		let refused = client
			.create_conversation_in(
				Uuid::now_v7(),
				RetentionPolicy::Retain,
				In::Workspace {
					project_id: project.project_id,
					base,
					seed,
				},
			)
			.await
			.unwrap_err();
		let ClientError::Remote(refusal) = refused else {
			panic!("expected a stable remote error, got {refused:?}");
		};
		refusals.push((refusal.category, refusal.code));
	}
	let mut older = hello(Uuid::new_v4());
	older.minor = WORKSPACES_MINOR;
	let (mut connection, _) = handshake_raw(&daemon, &older).await;
	connection
		.send(&ClientMessage::Command {
			id: 1,
			command_id: Uuid::now_v7(),
			command: CommandRequest::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: In::Workspace {
					project_id: project.project_id,
					base: BaseSelection::Head,
					seed: SeedSelection::AllEligible,
				},
			},
		})
		.await;
	let ServerMessage::Error { error, .. } = connection.receive().await else {
		panic!("expected a refusal");
	};
	let conversations = client.conversations().await.unwrap();

	assert_eq!(
		(
			refusals,
			error.code.as_str(),
			error.message.as_str(),
			conversations.conversations.len(),
		),
		(
			vec![
				(ErrorCategory::InvalidInput, "path.parent_traversal".into()),
				(
					ErrorCategory::NotFound,
					"workspace.seed_path_not_found".into()
				),
			],
			"protocol.unsupported_minor",
			"a Workspace seeded from the Local checkout needs protocol minor 10",
			0,
		)
	);
}
