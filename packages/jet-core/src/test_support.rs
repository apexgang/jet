//! Helpers shared by the core's test modules.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use jet_store::Store;
use uuid::Uuid;

use crate::capability::{
	CapabilityProbe, CredentialStoreKind, CredentialStoreStatus, ExternalTool,
	ExternalToolStatus, InstalledCraft, ObservedCapabilities, Platform,
	ToolAvailability,
};
use crate::clock::{Clock, SystemClock};
use crate::{
	Actor, ClientId, Command, CommandEnvelope, CommandId, Core, CraftId,
	HarnessId,
};

/// The one interactive Actor every core test acts as.
pub(crate) fn actor() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::nil()),
	}
}

/// Starts a core over a fresh or existing store at `path`, on a Plane that
/// has everything. Tests observe a fixed Plane rather than the machine they
/// run on, so no test depends on which tools the host installed.
pub(crate) async fn start_core(path: &Path) -> Core {
	start_core_with(path, Arc::new(SystemClock), FixedProbe::new(equipped()))
		.await
}

/// The same core with an injected clock or Plane observation.
pub(crate) async fn start_core_with(
	path: &Path,
	clock: Arc<dyn Clock>,
	probe: Arc<FixedProbe>,
) -> Core {
	let store = Store::open(path).await.unwrap();
	Core::start_with(store, clock, probe).await.unwrap()
}

/// A clock a test moves by hand, so a retention window or an observation
/// has an exact time instead of whatever the machine's clock said.
#[derive(Debug)]
pub(crate) struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
	pub(crate) fn at(now: SystemTime) -> Arc<Self> {
		Arc::new(Self(Mutex::new(now)))
	}

	pub(crate) fn advance(&self, duration: Duration) {
		let mut now = self.0.lock().unwrap();
		*now += duration;
	}
}

impl Clock for ManualClock {
	fn now(&self) -> SystemTime {
		*self.0.lock().unwrap()
	}
}

/// A Capability probe whose answer a test changes between observations, the
/// way a Plane changes when a tool is uninstalled while `jetd` runs.
#[derive(Debug)]
pub(crate) struct FixedProbe(Mutex<ObservedCapabilities>);

impl FixedProbe {
	pub(crate) fn new(observed: ObservedCapabilities) -> Arc<Self> {
		Arc::new(Self(Mutex::new(observed)))
	}

	pub(crate) fn answer_with(&self, observed: ObservedCapabilities) {
		*self.0.lock().unwrap() = observed;
	}
}

impl CapabilityProbe for FixedProbe {
	fn observe(
		&self,
	) -> Pin<Box<dyn Future<Output = ObservedCapabilities> + Send + '_>> {
		let observed = self.0.lock().unwrap().clone();
		Box::pin(async move { observed })
	}
}

/// A Plane with every tool, a reachable credential store, and one Craft.
pub(crate) fn equipped() -> ObservedCapabilities {
	ObservedCapabilities {
		platform: Platform {
			operating_system: "linux",
			architecture: "aarch64",
		},
		external_tools: vec![
			tool(ExternalTool::Git, present()),
			tool(ExternalTool::GitLfs, present()),
			tool(ExternalTool::Ssh, present()),
			tool(ExternalTool::Tailscale, present()),
		],
		credential_store: CredentialStoreStatus::Available {
			kind: CredentialStoreKind::SecretService,
		},
		crafts: vec![InstalledCraft {
			craft: CraftId("jet-craft-codex".into()),
			version: "0.2.0".into(),
			harnesses: vec![HarnessId("codex".into())],
		}],
	}
}

/// The same Plane whose credential store is present but locked, so it
/// answers nothing until the user unlocks it (ADR-0076).
pub(crate) fn locked() -> ObservedCapabilities {
	ObservedCapabilities {
		credential_store: CredentialStoreStatus::Locked {
			kind: CredentialStoreKind::SecretService,
		},
		..equipped()
	}
}

/// The same Plane after Git, Git LFS, and Tailscale were uninstalled, its
/// Craft removed, and its session bus lost.
pub(crate) fn stripped() -> ObservedCapabilities {
	ObservedCapabilities {
		external_tools: vec![
			tool(ExternalTool::Git, ToolAvailability::Missing),
			tool(ExternalTool::GitLfs, ToolAvailability::Missing),
			tool(ExternalTool::Ssh, present()),
			tool(ExternalTool::Tailscale, ToolAvailability::Missing),
		],
		credential_store: CredentialStoreStatus::Unavailable {
			kind: CredentialStoreKind::SecretService,
		},
		crafts: Vec::new(),
		..equipped()
	}
}

fn tool(
	tool: ExternalTool,
	availability: ToolAvailability,
) -> ExternalToolStatus {
	ExternalToolStatus { tool, availability }
}

fn present() -> ToolAvailability {
	ToolAvailability::Present {
		version: "2.51.0".into(),
	}
}

/// A fresh Command identity for a request that is not a retry.
pub(crate) fn command_id() -> CommandId {
	CommandId(Uuid::now_v7())
}

/// Wraps `command` in an envelope bound to its own encoded bytes, the way
/// `jetd` binds the bytes a client sent.
pub(crate) fn request(command: Command) -> CommandEnvelope {
	request_with_id(command_id(), command)
}

/// The same envelope under a chosen identity, so a test can retry one
/// Command exactly.
pub(crate) fn request_with_id(
	command_id: CommandId,
	command: Command,
) -> CommandEnvelope {
	let bytes = serde_json::to_vec(&command).unwrap();
	CommandEnvelope::new(command_id, command, &bytes).unwrap()
}

/// Runs one `git` command in `dir` with the configuration a test needs and
/// nothing the host's own configuration could add, and returns what it
/// printed. Tests that touch repositories need `git` on the host, as CI
/// provisions it (ADR-0056).
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
	let output = std::process::Command::new("git")
		.env("GIT_CONFIG_NOSYSTEM", "1")
		.env("GIT_CONFIG_GLOBAL", "/dev/null")
		.env("LC_ALL", "C")
		.arg("-C")
		.arg(dir)
		.args([
			"-c",
			"user.name=Jet",
			"-c",
			"user.email=jet@example.invalid",
			"-c",
			"init.defaultBranch=main",
			"-c",
			"commit.gpgsign=false",
			"-c",
			"protocol.file.allow=always",
		])
		.args(args)
		.output()
		.expect("git runs on the test host");
	assert!(
		output.status.success(),
		"git {args:?} in {} failed: {}",
		dir.display(),
		String::from_utf8_lossy(&output.stderr)
	);
	String::from_utf8(output.stdout).unwrap()
}

/// Creates an ordinary repository at `dir` with one commit and returns its
/// canonical path, which on macOS differs from the temporary path a test
/// was handed.
pub(crate) fn init_repository(dir: &Path) -> std::path::PathBuf {
	std::fs::create_dir_all(dir).unwrap();
	git(dir, &["init", "-q"]);
	std::fs::write(dir.join("README.md"), "# Jet\n").unwrap();
	git(dir, &["add", "-A"]);
	git(dir, &["commit", "-q", "-m", "Initial"]);
	dir.canonicalize().unwrap()
}
