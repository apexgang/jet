//! The GUI-managed core lifecycle through the public `jetd core` surface:
//! staging, activation, drain with live work, rollback, pruning, and the
//! refusals that protect the release pair (ADR-0026, ADR-0073, ADR-0088).
#[path = "support/run_assertions.rs"]
mod assertions;
#[path = "support/run_fixture.rs"]
mod fixture;
mod support;
use assertions::wait_for;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
	path::{Path, PathBuf},
	process::Stdio,
	time::Duration,
};
use support::{connect, connect_raw, init_repository, start_jetd};
use uuid::Uuid;

const EXECUTABLES: [&str; 4] =
	["jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"];

/// Runs `jetd core` with `args` and returns its exit code and report.
fn core(args: &[&str]) -> (i32, Value) {
	let (code, report) = core_output(args);
	(
		code,
		report.unwrap_or_else(|stderr| {
			panic!("jetd core {args:?} printed no report: {stderr}")
		}),
	)
}

/// Runs `jetd core` with `args`; a failure prints no report, only stderr.
fn core_output(args: &[&str]) -> (i32, Result<Value, String>) {
	let output = std::process::Command::new(env!("CARGO_BIN_EXE_jetd"))
		.arg("core")
		.args(args)
		.output()
		.unwrap();
	let stdout = String::from_utf8(output.stdout).unwrap();
	let report = serde_json::from_str(stdout.trim())
		.map_err(|_| String::from_utf8_lossy(&output.stderr).into_owned());
	(output.status.code().unwrap(), report)
}

/// This build's release identity, as `jetd core describe` reports it.
fn describe() -> Value {
	core(&["describe"]).1
}

/// Writes a payload of this build's real executables under `version`,
/// letting `edit` adjust the manifest before it is written.
fn payload(
	dir: &Path,
	version: &str,
	edit: impl FnOnce(&mut Value),
) -> PathBuf {
	let dir = dir.join(format!("payload-{version}"));
	std::fs::create_dir_all(&dir).unwrap();
	let mut executables = serde_json::Map::new();
	for name in EXECUTABLES {
		let source = Path::new(env!("CARGO_BIN_EXE_jetd")).with_file_name(name);
		std::fs::copy(&source, dir.join(name)).unwrap();
		let digest = Sha256::digest(std::fs::read(&source).unwrap());
		executables.insert(name.to_owned(), json!(format!("{digest:x}")));
	}
	let identity = describe();
	let mut manifest = json!({
		"version": version,
		"target": "test-target",
		"protocol_major": identity["protocol_major"],
		"protocol_minor": identity["protocol_minor"],
		"schema_version": identity["schema_version"],
		"executables": executables,
	});
	edit(&mut manifest);
	std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
	dir
}

fn stage(home: &Path, payload: &Path) -> (i32, Value) {
	core(&[
		"stage",
		"--home",
		home.to_str().unwrap(),
		"--payload",
		payload.to_str().unwrap(),
	])
}

fn status(home: &Path) -> Value {
	core(&["status", "--home", home.to_str().unwrap()]).1
}

#[tokio::test]
async fn versions_are_staged_activated_rolled_back_and_pruned() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	let home_arg = home.to_str().unwrap();
	let first = payload(dir.path(), "0.2.0-test.1", |_| {});
	let second = payload(dir.path(), "0.2.0-test.2", |_| {});
	let third = payload(dir.path(), "0.2.0-test.3", |_| {});

	let staged = stage(&home, &first);
	let activated =
		core(&["activate", "--home", home_arg, "--version", "0.2.0-test.1"]);
	stage(&home, &second);
	let updated =
		core(&["activate", "--home", home_arg, "--version", "0.2.0-test.2"]);
	let rolled_back = core(&["rollback", "--home", home_arg]);
	let after_rollback = status(&home);
	stage(&home, &third);
	core(&["activate", "--home", home_arg, "--version", "0.2.0-test.3"]);
	let pruned = core(&["prune", "--home", home_arg]);
	let unstaged = core_output(&[
		"activate",
		"--home",
		home_arg,
		"--version",
		"0.2.0-test.2",
	]);
	let executable = std::fs::metadata(home.join("core/current/jetd")).unwrap();
	let owner_executable =
		std::os::unix::fs::PermissionsExt::mode(&executable.permissions())
			& 0o777;

	assert_eq!(
		(staged, activated, updated, rolled_back, after_rollback, pruned, unstaged, status(&home)["staged"].clone(), owner_executable),
		(
			(0, json!({"status": "staged", "version": "0.2.0-test.1", "target": "test-target"})),
			(0, json!({"status": "activated", "version": "0.2.0-test.1", "previous": null, "drained": null, "live_helpers": 0})),
			(0, json!({"status": "activated", "version": "0.2.0-test.2", "previous": "0.2.0-test.1", "drained": null, "live_helpers": 0})),
			(0, json!({"status": "activated", "version": "0.2.0-test.1", "previous": "0.2.0-test.2", "drained": null, "live_helpers": 0})),
			json!({
				"status": "ok",
				"current": "0.2.0-test.1",
				"previous": "0.2.0-test.2",
				"staged": ["0.2.0-test.1", "0.2.0-test.2"],
				"live_helpers": [],
				"owner": {"state": "free"},
			}),
			(0, json!({"status": "pruned", "removed": ["0.2.0-test.2"]})),
			(1, Err("jetd core: version 0.2.0-test.2 is not staged: No such file or directory (os error 2)\n".into())),
			json!(["0.2.0-test.1", "0.2.0-test.3"]),
			0o500,
		)
	);
}

#[tokio::test]
async fn activation_drains_the_owner_and_the_run_survives_into_the_new_version()
{
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		let home_arg = home.to_str().unwrap();
		fixture::install(&home);
		let mut daemon = start_jetd(&home).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap().to_owned();
		let waiting = wait_for(&mut wire, &run_id, "waiting_for_approval").await;
		let helper_pid = status(&home)["live_helpers"][0]["pid"].as_u64().unwrap();
		let staged = payload(dir.path(), "0.2.0-test.1", |_| {});
		stage(&home, &staged);
		let pid = daemon.child.id().unwrap();

		let switched = core(&["activate", "--home", home_arg, "--version", "0.2.0-test.1", "--replace-channel", "development"]);
		let exit = daemon.child.wait().await.unwrap();
		let helper_alive = rustix::process::test_kill_process(
			rustix::process::Pid::from_raw(i32::try_from(helper_pid).unwrap()).unwrap(),
		)
		.is_ok();

		assert_eq!(
			(switched, exit.code(), helper_alive),
			(
				(0, json!({
					"status": "activated",
					"version": "0.2.0-test.1",
					"previous": null,
					"drained": {"pid": pid, "version": describe()["version"], "channel": "development"},
					"live_helpers": 1,
				})),
				Some(0),
				true,
			)
		);

		// The service manager's job: start the daemon `current` now names.
		std::fs::write(root.join("continue"), "go").unwrap();
		let mut command = tokio::process::Command::new(home.join("core/current/jetd"));
		command.arg("serve").arg("--home").arg(&home).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
		let daemon = support::start_jetd_process(&mut command).await;
		let mut wire = connect_raw(&daemon, client_id).await;
		let completed = wait_for(&mut wire, &run_id, "completed").await;
		let mut expected_processes = waiting["processes"].clone();
		for process in expected_processes.as_array_mut().unwrap() { process["running"] = json!(false); }
		assert_eq!((completed["processes"].clone(), completed["exit_code"].clone()), (expected_processes, json!(0)));
	}).await.unwrap();
}

#[tokio::test]
async fn activation_is_refused_outside_the_pair_and_across_channels() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		let home_arg = home.to_str().unwrap();
		fixture::install(&home);
		let identity = describe();
		let schema = identity["schema_version"].as_i64().unwrap();
		let current = payload(dir.path(), "0.2.0-test.1", |_| {});
		let next = payload(dir.path(), "0.2.0-test.2", |_| {});
		let major = payload(dir.path(), "0.2.0-test.3", |m| m["protocol_major"] = json!(2));
		let older = payload(dir.path(), "0.1.0-test", |m| m["schema_version"] = json!(schema - 1));
		let foreign = payload(dir.path(), "0.2.0-test.4", |m| m["target"] = json!("other-target"));
		for staged in [&current, &next, &major, &older, &foreign] {
			assert_eq!(stage(&home, staged).0, 0);
		}
		core(&["activate", "--home", home_arg, "--version", "0.2.0-test.1"]);
		let mut daemon = support::start_jetd_process(support::jetd(&home).arg("--channel").arg("homebrew")).await;
		let client_id = Uuid::new_v4();
		let client = connect(&daemon, client_id).await;
		let root = init_repository(&dir.path().join("repo"));
		let project = client.register_project(Uuid::now_v7(), root.to_str().unwrap()).await.unwrap();
		let conversation = client.create_conversation_in(Uuid::now_v7(), jet_protocol::RetentionPolicy::Retain,
			jet_protocol::WorkingTreeRequest::LocalCheckout { project_id: project.project_id }).await.unwrap();
		let mut wire = connect_raw(&daemon, client_id).await;
		wire.send(&json!({"kind":"command","id":1,"command_id":Uuid::now_v7(),"command":{"type":"start_run","conversation_id":conversation.conversation_id,"craft":"fake","prompt":"Make a change"}})).await;
		let admitted: Value = wire.receive().await;
		let run_id = admitted["result"]["run_id"].as_str().unwrap().to_owned();
		wait_for(&mut wire, &run_id, "waiting_for_approval").await;
		let pid = daemon.child.id().unwrap();

		let other_channel = core(&["activate", "--home", home_arg, "--version", "0.2.0-test.2"]);
		let pinned_major = core(&["activate", "--home", home_arg, "--version", "0.2.0-test.3", "--replace-channel", "homebrew"]);
		let outside_pair = core(&["activate", "--home", home_arg, "--version", "0.1.0-test", "--replace-channel", "homebrew"]);
		let other_target = core(&["activate", "--home", home_arg, "--version", "0.2.0-test.4", "--replace-channel", "homebrew"]);
		let still_owned = rustix::process::test_kill_process(rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap()).is_ok();

		assert_eq!(
			(other_channel, pinned_major, outside_pair, other_target, still_owned, status(&home)["current"].clone()),
			(
				(3, json!({"status": "refused", "code": "channel_owned", "owner": {"pid": pid, "version": identity["version"], "channel": "homebrew"}})),
				(3, json!({"status": "refused", "code": "protocol_major_pinned", "candidate": 2, "current": identity["protocol_major"], "live_helpers": 1})),
				(3, json!({"status": "refused", "code": "schema_outside_pair", "candidate": schema - 1, "store": schema, "previous": null})),
				(3, json!({"status": "refused", "code": "target_mismatch", "candidate": "other-target", "current": "test-target"})),
				true,
				json!("0.2.0-test.1"),
			)
		);

		std::fs::write(root.join("continue"), "go").unwrap();
		wait_for(&mut wire, &run_id, "completed").await;
		daemon.child.kill().await.unwrap();
	}).await.unwrap();
}
