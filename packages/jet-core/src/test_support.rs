//! Helpers shared by the core's test modules.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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

/// The same Plane after Git and Tailscale were uninstalled, its Craft
/// removed, and its session bus lost.
pub(crate) fn stripped() -> ObservedCapabilities {
	ObservedCapabilities {
		external_tools: vec![
			tool(ExternalTool::Git, ToolAvailability::Missing),
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
