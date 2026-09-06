//! Black-box Project registration conformance tests at the public Jet
//! protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};

use jet_client::ClientError;
use std::os::unix::fs::symlink;

use jet_protocol::{
	Actor, CapabilityObservation, Checkout, ClientMessage, EntryKind,
	ErrorCategory, ExternalTool, PAIRING_MINOR, Project, ProjectEntry,
	ProjectPreview, QueryRequest, Registrability, Repository, ServerHello,
	ServerMessage, Worktree,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, start_jetd};
use uuid::Uuid;

/// Creates an ordinary repository at `dir` with one commit and returns its
/// canonical path. The test host needs `git`, as CI provisions it
/// (ADR-0056).
fn init_repository(dir: &Path) -> PathBuf {
	std::fs::create_dir_all(dir).unwrap();
	for args in [
		vec!["init", "-q"],
		vec!["add", "-A"],
		vec!["commit", "-q", "--allow-empty", "-m", "Initial"],
	] {
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
			.args(&args)
			.output()
			.unwrap();
		assert!(
			output.status.success(),
			"git {args:?} failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
	}
	dir.canonicalize().unwrap()
}

/// A Path grant is the one request that carries an absolute path, and the
/// Project it registers is Plane state: it names the canonical root, the
/// Actor that granted it, and outlives the daemon (ADR-0025, ADR-0101).
#[tokio::test]
async fn a_registered_project_outlives_the_daemon_that_registered_it() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let repository = init_repository(&dir.path().join("repo"));

	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let registered = client
		.register_project(
			Uuid::now_v7(),
			dir.path().join("repo").to_str().unwrap(),
		)
		.await
		.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let listed = client.projects().await.unwrap();

	assert_eq!(
		(&registered, listed.projects),
		(
			&Project {
				project_id: registered.project_id,
				root: repository.display().to_string(),
				registered_by: Actor::InteractiveClient { client_id },
				registered_at_unix_ms: registered.registered_at_unix_ms,
			},
			vec![registered.clone()]
		)
	);
}

/// The Plane refuses a grant that is not an ordinary working tree, or not
/// an absolute path at all, with a stable code and nothing registered
/// (ADR-0068, ADR-0103).
#[tokio::test]
async fn a_grant_that_is_not_a_working_tree_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let plain = dir.path().join("plain");
	std::fs::create_dir_all(&plain).unwrap();

	let mut refused = Vec::new();
	for path in [plain.to_str().unwrap(), "repo", "/tmp/jet\0repo"] {
		let error = client
			.register_project(Uuid::now_v7(), path)
			.await
			.unwrap_err();
		let ClientError::Remote(error) = error else {
			panic!("expected a stable remote error, got {error:?}");
		};
		refused.push((error.category, error.code));
	}
	let listed = client.projects().await.unwrap();

	assert_eq!(
		(refused, listed.projects),
		(
			vec![
				(
					ErrorCategory::InvalidInput,
					"project.not_a_repository".into()
				),
				(
					ErrorCategory::InvalidInput,
					"path_grant.not_absolute".into()
				),
				(ErrorCategory::InvalidInput, "path_grant.nul".into()),
			],
			vec![]
		)
	);
}

/// A client that negotiated a minor without Projects is answered with a
/// stable refusal rather than a guess (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_project_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = PAIRING_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == PAIRING_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::Projects,
		})
		.await;

	let ServerMessage::Error { id, error } = connection.receive().await else {
		panic!("expected a refusal");
	};
	assert_eq!(
		(
			id,
			error.category,
			error.code.as_str(),
			error.message.as_str()
		),
		(
			Some(1),
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			"the Project Query needs protocol minor 8"
		)
	);
}

/// A preview is the look before the grant (ADR-0101): it names the
/// directory the path resolves to and what the Plane's Git makes of it,
/// and registers nothing.
#[tokio::test]
async fn a_preview_describes_a_working_tree_without_registering_it() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	std::fs::create_dir_all(repository.join("src")).unwrap();
	let lfs = client
		.capabilities(CapabilityObservation::LastObserved)
		.await
		.unwrap()
		.external_tools
		.into_iter()
		.find(|status| status.tool == ExternalTool::GitLfs)
		.unwrap()
		.availability;

	let previewed = client
		.preview_project(
			dir.path().join("repo").to_str().unwrap(),
			CapabilityObservation::LastObserved,
		)
		.await
		.unwrap();
	let inside = client
		.preview_project(
			repository.join("src").to_str().unwrap(),
			CapabilityObservation::LastObserved,
		)
		.await
		.unwrap();
	let listed = client.projects().await.unwrap();

	assert_eq!(
		(previewed, inside.registrability, listed.projects),
		(
			ProjectPreview {
				root: repository.display().to_string(),
				registrability: Registrability::Registrable {
					repository: Repository {
						worktree: Worktree::Main,
						checkout: Checkout::Full,
						submodules: vec![],
						lfs,
					},
				},
			},
			Registrability::InsideWorkingTree {
				toplevel: repository.display().to_string(),
			},
			vec![]
		)
	);
}

/// Every ordinary file operation names a Project and a relative path
/// (ADR-0101). An absolute path, a parent traversal, a NUL, a form another
/// platform reads differently, and a link that leaves the root are each
/// refused with a stable code before the Plane touches anything.
#[tokio::test]
async fn a_file_is_addressed_through_its_project_and_a_relative_path() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let repository = init_repository(&dir.path().join("repo"));
	std::fs::write(repository.join("notes.md"), "hello\n").unwrap();
	symlink(dir.path(), repository.join("escape")).unwrap();
	let project = client
		.register_project(Uuid::now_v7(), repository.to_str().unwrap())
		.await
		.unwrap();

	let notes = client
		.project_entry(project.project_id, "notes.md")
		.await
		.unwrap();
	let mut refused = Vec::new();
	for path in [
		"/etc/passwd",
		"../secrets",
		"a\0b",
		"src\\main.rs",
		"escape/plane.sqlite3",
	] {
		let error = client
			.project_entry(project.project_id, path)
			.await
			.unwrap_err();
		let ClientError::Remote(error) = error else {
			panic!("expected a stable remote error, got {error:?}");
		};
		refused.push((error.category, error.code));
	}

	let expected = ProjectEntry {
		cursor: notes.cursor,
		project_id: project.project_id,
		path: "notes.md".into(),
		kind: EntryKind::File { bytes: 6 },
	};
	assert_eq!(
		(notes, refused),
		(
			expected,
			vec![
				(ErrorCategory::InvalidInput, "path.absolute".into()),
				(ErrorCategory::InvalidInput, "path.parent_traversal".into()),
				(ErrorCategory::InvalidInput, "path.nul".into()),
				(ErrorCategory::InvalidInput, "path.platform_form".into()),
				(ErrorCategory::InvalidInput, "path.escapes_root".into()),
			]
		)
	);
}
