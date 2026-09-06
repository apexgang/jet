//! Black-box tests for Workspace promotion at the public Jet protocol
//! boundary (ADR-0025).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;

use jet_client::{Client, ClientError};
use jet_protocol::{
	BaseSelection, ChangeKind, ClientMessage, CommandRequest, ConflictKind,
	ErrorCategory, PromotedChange, PromotionBinding, PromotionConflict,
	PromotionDestination, PromotionPreview, PromotionState, QueryRequest,
	RetentionPolicy, SEEDED_WORKSPACES_MINOR, SeedSelection, ServerMessage,
	WorkingTreeRequest, Workspace, WorkspacePromotion,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, init_repository, start_jetd};
use uuid::Uuid;

/// Runs one `git` command in `dir` and returns what it printed.
fn git(dir: &Path, args: &[&str]) -> String {
	let output = std::process::Command::new("git")
		.env("GIT_CONFIG_NOSYSTEM", "1")
		.env("GIT_CONFIG_GLOBAL", "/dev/null")
		.arg("-C")
		.arg(dir)
		.args([
			"-c",
			"user.name=Jet",
			"-c",
			"user.email=jet@example.invalid",
			"-c",
			"commit.gpgsign=false",
		])
		.args(args)
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"git {args:?} failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	String::from_utf8(output.stdout).unwrap()
}

/// A registered Project at `repository` with `f.txt` committed, and a
/// Workspace of it that rewrote the first line of `f.txt` and added
/// `new.txt`.
async fn workspace_with_changes(
	client: &Client,
	repository: &Path,
) -> Workspace {
	std::fs::write(repository.join("f.txt"), "a\nb\nc\n").unwrap();
	git(repository, &["add", "-A"]);
	git(repository, &["commit", "-q", "-m", "Base"]);
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();
	let conversation = client
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
	let workspace = client
		.conversation(conversation.conversation_id)
		.await
		.unwrap()
		.workspace
		.unwrap();
	let root = Path::new(&workspace.root);
	std::fs::write(root.join("f.txt"), "A\nb\nc\n").unwrap();
	std::fs::write(root.join("new.txt"), "new\n").unwrap();
	workspace
}

/// A preview merges the Workspace over the Local checkout's own changes,
/// lists what would change, names what it cannot settle, binds what it
/// compared to the client that asked, and changes nothing (ADR-0025).
#[tokio::test]
async fn a_promotion_preview_shows_the_merge_and_changes_nothing() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client_id = Uuid::new_v4();
	let client = connect(&daemon, client_id).await;
	let repository = init_repository(&dir.path().join("repo"));
	let workspace = workspace_with_changes(&client, &repository).await;
	let base = git(&repository, &["rev-parse", "HEAD"]).trim().to_owned();
	std::fs::write(repository.join("f.txt"), "a\nb\nC\n").unwrap();

	let clean = client
		.preview_promotion(
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();
	std::fs::write(repository.join("new.txt"), "mine\n").unwrap();
	let conflicted = client
		.preview_promotion(
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();

	let binding = clean.binding.clone();
	assert_eq!(
		(
			&clean,
			conflicted.conflicts,
			std::fs::read_to_string(repository.join("f.txt")).unwrap(),
			git(&repository, &["status", "--porcelain"]),
		),
		(
			&PromotionPreview {
				cursor: clean.cursor,
				binding: PromotionBinding {
					workspace_id: workspace.workspace_id,
					destination: PromotionDestination::LocalCheckout,
					base_commit: base.clone(),
					workspace_tree: binding.workspace_tree.clone(),
					destination_commit: base,
					destination_tree: binding.destination_tree.clone(),
					result_tree: binding.result_tree.clone(),
					actor: client_id,
				},
				destination_dirty: true,
				changed_paths: 2,
				changes: vec![
					PromotedChange {
						path: "f.txt".into(),
						kind: ChangeKind::Modified,
					},
					PromotedChange {
						path: "new.txt".into(),
						kind: ChangeKind::Added,
					},
				],
				conflicts: vec![],
			},
			vec![PromotionConflict {
				path: "new.txt".into(),
				kind: ConflictKind::Diverged,
			}],
			"a\nb\nC\n".into(),
			" M f.txt\n?? new.txt\n".into(),
		)
	);
}

/// A destination the Plane cannot promote to is refused with a stable
/// error, and a client that negotiated a minor without promotion is
/// refused the preview (ADR-0019, ADR-0025).
#[tokio::test]
async fn a_preview_the_plane_cannot_give_is_refused_with_a_stable_error() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let workspace = workspace_with_changes(&client, &repository).await;
	let mut refusals = Vec::new();
	for destination in [
		PromotionDestination::Branch {
			name: "nowhere".into(),
		},
		PromotionDestination::Branch {
			name: git(&repository, &["branch", "--show-current"])
				.trim()
				.to_owned(),
		},
	] {
		let refused = client
			.preview_promotion(workspace.workspace_id, destination)
			.await
			.unwrap_err();
		let ClientError::Remote(refusal) = refused else {
			panic!("expected a stable remote error, got {refused:?}");
		};
		refusals.push((refusal.category, refusal.code));
	}
	let previewed = client
		.preview_promotion(
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();
	let mut older = hello(Uuid::new_v4());
	older.minor = SEEDED_WORKSPACES_MINOR;
	let (mut connection, _) = handshake_raw(&daemon, &older).await;
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::PreviewPromotion {
				workspace_id: workspace.workspace_id,
				destination: PromotionDestination::LocalCheckout,
			},
		})
		.await;
	let ServerMessage::Error { error, .. } = connection.receive().await else {
		panic!("expected a refusal");
	};
	connection
		.send(&ClientMessage::Command {
			id: 2,
			command_id: Uuid::now_v7(),
			command: CommandRequest::PromoteWorkspace {
				binding: previewed.binding,
			},
		})
		.await;
	let ServerMessage::Error {
		error: refused_command,
		..
	} = connection.receive().await
	else {
		panic!("expected a refusal");
	};

	assert_eq!(
		(
			refusals,
			error.code.as_str(),
			error.message.as_str(),
			refused_command.message.as_str(),
		),
		(
			vec![
				(
					ErrorCategory::NotFound,
					"workspace.promotion_branch_not_found".into()
				),
				(
					ErrorCategory::Conflict,
					"workspace.promotion_branch_checked_out".into()
				),
			],
			"protocol.unsupported_minor",
			"the Workspace promotion preview Query needs protocol minor 11",
			"Workspace promotion needs protocol minor 11",
		)
	);
}

/// A promotion confirmed from a conflicted preview is recorded with its
/// conflicts, shown on the Workspace, and writes nothing; a stale binding
/// is refused; and a clean binding is recorded as applying (ADR-0025).
#[tokio::test]
async fn a_promotion_is_recorded_as_previewed_or_refused_as_stale() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	let workspace = workspace_with_changes(&client, &repository).await;
	std::fs::write(repository.join("f.txt"), "X\nb\nc\n").unwrap();
	let conflicted = client
		.preview_promotion(
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();

	let recorded = client
		.promote_workspace(Uuid::now_v7(), conflicted.binding.clone())
		.await
		.unwrap();
	let shown = client
		.conversation(workspace.conversation_id)
		.await
		.unwrap()
		.workspace
		.unwrap()
		.promotion;
	std::fs::write(repository.join("f.txt"), "a\nb\nc\n").unwrap();
	let stale = client
		.promote_workspace(Uuid::now_v7(), conflicted.binding.clone())
		.await
		.unwrap_err();
	let ClientError::Remote(stale) = stale else {
		panic!("expected a stable remote error, got {stale:?}");
	};
	let clean = client
		.preview_promotion(
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();
	let applying = client
		.promote_workspace(Uuid::now_v7(), clean.binding.clone())
		.await
		.unwrap();

	assert_eq!(
		(
			&recorded,
			shown.as_ref(),
			(stale.category, stale.code.as_str()),
			(applying.state, applying.conflicts.len()),
		),
		(
			&WorkspacePromotion {
				promotion_id: recorded.promotion_id,
				binding: conflicted.binding,
				changed_paths: 2,
				state: PromotionState::Conflicted,
				conflicts: vec![PromotionConflict {
					path: "f.txt".into(),
					kind: ConflictKind::Diverged,
				}],
				recorded_at_unix_ms: recorded.recorded_at_unix_ms,
				settled_at_unix_ms: Some(recorded.recorded_at_unix_ms),
			},
			Some(&recorded),
			(ErrorCategory::Conflict, "workspace.promotion_stale"),
			(PromotionState::Applying, 0),
		)
	);
}
