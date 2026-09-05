//! Black-box Project registration conformance tests at the public Jet
//! protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};

use jet_client::ClientError;
use jet_protocol::{
	Actor, ClientMessage, ErrorCategory, PAIRING_MINOR, Project, QueryRequest,
	ServerHello, ServerMessage,
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
