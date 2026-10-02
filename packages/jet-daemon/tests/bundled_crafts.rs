//! The Bundled Crafts of a release payload are installed Crafts as soon as
//! `jetd` serves from it, through the GUI-managed `~/.jet/core` layout and
//! through a Homebrew-shaped keg alike (ADR-0107).

mod support;

use jet_protocol::{CapabilitySnapshot, CraftSpecification, DegradedCondition};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
	path::{Path, PathBuf},
	process::Stdio,
	time::Duration,
};

const EXECUTABLES: [&str; 4] =
	["jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"];

/// Runs `jetd core` with `args` and returns its report.
fn core(args: &[&str]) -> Value {
	let output = std::process::Command::new(env!("CARGO_BIN_EXE_jetd"))
		.arg("core")
		.args(args)
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"jetd core {args:?} failed: {output:?}"
	);
	serde_json::from_slice(&output.stdout).unwrap()
}

/// Writes this build's real executables and a manifest pinning them, as a
/// release payload of this build's own version, into `dir`.
fn payload(dir: &Path) -> String {
	std::fs::create_dir_all(dir).unwrap();
	let mut executables = serde_json::Map::new();
	for name in EXECUTABLES {
		let source = Path::new(env!("CARGO_BIN_EXE_jetd")).with_file_name(name);
		std::fs::copy(&source, dir.join(name)).unwrap();
		let digest = Sha256::digest(std::fs::read(&source).unwrap());
		executables.insert(name.to_owned(), json!(hex::encode(digest)));
	}
	let identity = core(&["describe"]);
	let manifest = json!({
		"version": identity["version"],
		"target": "test-target",
		"protocol_major": identity["protocol_major"],
		"protocol_minor": identity["protocol_minor"],
		"schema_version": identity["schema_version"],
		"executables": executables,
	});
	std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
	identity["version"].as_str().unwrap().to_owned()
}

async fn serve(executable: &Path, home: &Path) -> support::Daemon {
	let mut command = tokio::process::Command::new(executable);
	command
		.arg("serve")
		.arg("--home")
		.arg(home)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true);
	support::start_jetd_process(&mut command).await
}

/// Each listed Craft as its id, version, and Harnesses, plus whether the
/// Plane still says it can run no Harness.
fn listed(
	daemon: &support::Daemon,
) -> (Vec<(String, String, Vec<String>)>, bool) {
	let capabilities: CapabilitySnapshot =
		serde_json::from_value(daemon.ready["capabilities"].clone()).unwrap();
	(
		capabilities
			.crafts
			.into_iter()
			.map(|craft| (craft.craft_id, craft.version, craft.harnesses))
			.collect(),
		capabilities
			.degraded
			.contains(&DegradedCondition::NoHarnessAvailable),
	)
}

fn both(version: &str) -> (Vec<(String, String, Vec<String>)>, bool) {
	(
		vec![
			(
				"claude-code".into(),
				version.into(),
				vec!["claude-code".into()],
			),
			("codex".into(), version.into(), vec!["codex".into()]),
		],
		false,
	)
}

fn manifest(home: &Path, id: &str) -> Value {
	serde_json::from_slice(
		&std::fs::read(home.join(format!("crafts/{id}.json"))).unwrap(),
	)
	.unwrap()
}

/// What a Run pins from the registration must be the Artifact the release
/// pinned and the declaration the Craft presents at its handshake.
fn assert_registered(home: &Path, payload: &Path) {
	let release: Value = serde_json::from_slice(
		&std::fs::read(payload.join("manifest.json")).unwrap(),
	)
	.unwrap();
	for (executable, id, declaration) in [
		(
			"jet-craft-claude",
			"claude-code",
			include_str!("../../jet-craft-claude/.jet/craft-spec.toml"),
		),
		(
			"jet-craft-codex",
			"codex",
			include_str!("../../jet-craft-codex/.jet/craft-spec.toml"),
		),
	] {
		let registered = manifest(home, id);
		let digest = release["executables"][executable].as_str().unwrap();
		let specification: CraftSpecification =
			serde_json::from_value(registered["specification"].clone())
				.unwrap();
		assert_eq!(
			(
				registered["executable"].as_str().map(PathBuf::from),
				registered["sha256"].as_str(),
				registered["origin"].as_str(),
				specification,
			),
			(
				Some(
					home.join("crafts/artifacts")
						.join(digest)
						.canonicalize()
						.unwrap()
				),
				Some(digest),
				Some("bundled"),
				jet_craft_sdk::parse_specification(declaration).unwrap(),
			)
		);
	}
}

#[tokio::test]
async fn a_staged_release_serves_its_bundled_crafts_without_provisioning() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		let release = dir.path().join("payload");
		let version = payload(&release);
		let home_arg = home.to_str().unwrap();
		core(&[
			"stage",
			"--home",
			home_arg,
			"--payload",
			release.to_str().unwrap(),
		]);
		core(&["activate", "--home", home_arg, "--version", &version]);
		let current = home.join("core/current/jetd");

		let mut daemon = serve(&current, &home).await;
		assert_eq!(listed(&daemon), both(&version));
		assert_registered(&home, &release);
		let registered =
			[manifest(&home, "claude-code"), manifest(&home, "codex")];
		daemon.child.kill().await.unwrap();

		// A later start finds its release registered and changes nothing.
		let daemon = serve(&current, &home).await;
		assert_eq!(
			(
				listed(&daemon),
				[manifest(&home, "claude-code"), manifest(&home, "codex")]
			),
			(both(&version), registered)
		);
	})
	.await
	.unwrap();
}

#[tokio::test]
async fn a_homebrew_keg_serves_its_bundled_crafts_through_its_bin_symlink() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		let libexec = dir.path().join("prefix/libexec");
		let version = payload(&libexec);
		let bin = dir.path().join("prefix/bin");
		std::fs::create_dir_all(&bin).unwrap();
		std::os::unix::fs::symlink(libexec.join("jetd"), bin.join("jetd"))
			.unwrap();

		let daemon = serve(&bin.join("jetd"), &home).await;

		assert_eq!(listed(&daemon), both(&version));
		assert_registered(&home, &libexec);
	})
	.await
	.unwrap();
}
