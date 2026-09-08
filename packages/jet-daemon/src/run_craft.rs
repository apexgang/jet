//! Owner-provisioned Craft declarations translated into an opaque execution pin.
use crate::run_host::filesystem;
use jet_core::{CoreError, ForkLaunchSource, PinnedCraft};
use jet_protocol::{CraftSpecification, ProtocolVersion};
use serde::{Deserialize, Serialize};
use std::{
	io::Read,
	path::{Path, PathBuf},
};
#[derive(Deserialize)]
struct Installation {
	executable: PathBuf,
	sha256: String,
	specification: CraftSpecification,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Contract {
	pub(crate) version: u32,
	#[serde(default)]
	pub(crate) boot_identity: String,
	pub(crate) craft_protocol: ProtocolVersion,
	pub(crate) helper_protocol: ProtocolVersion,
	pub(crate) specification: CraftSpecification,
}
impl Contract {
	pub(crate) fn of(pin: &PinnedCraft) -> Result<Self, CoreError> {
		let contract: Self =
			jet_protocol::decode_control(pin.adapter_state.as_bytes())
				.map_err(|_| unavailable())?;
		if contract.version != 1
			|| !(1..=8).contains(&contract.craft_protocol.minor)
			|| contract.craft_protocol.major != 1
			|| contract.helper_protocol.major != 1
			|| contract.helper_protocol.minor > 1
		{
			return Err(unavailable());
		}
		Ok(contract)
	}
}

/// Only accepted Harness identities establish a known native Provider.
pub(crate) fn native_provider(
	pin: &PinnedCraft,
) -> Result<jet_core::ProviderId, CoreError> {
	match Contract::of(pin)?.specification.harness.as_str() {
		"codex" => Ok(jet_core::ProviderId("openai".into())),
		"claude-code" => Ok(jet_core::ProviderId("anthropic".into())),
		_ => Err(CoreError {
			code: "visa.provider_unavailable".into(),
			message: "the selected Harness's native Provider is unavailable"
				.into(),
			..unavailable()
		}),
	}
}
pub(crate) async fn load(
	home: &Path,
	id: &str,
) -> Result<PinnedCraft, CoreError> {
	// ASVS 5.3.2: Commands select installed identities, never paths.
	if id.is_empty()
		|| id.len() > 80
		|| !id
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
	{
		return Err(unavailable());
	}
	let path = home.join("crafts").join(format!("{id}.json"));
	let bytes = filesystem::blocking(move || bounded_read(&path, 65_536))
		.await?
		.map_err(|_| unavailable())?;
	let craft: Installation =
		jet_protocol::decode_control(&bytes).map_err(|_| unavailable())?;
	if craft.specification.id != id
		|| !craft
			.specification
			.enabled_features()
			.map_err(|_| unavailable())?
			.iter()
			.any(|f| f == "turns")
	{
		return Err(unavailable());
	}
	let offer = jet_protocol::ProtocolOffer {
		family: jet_protocol::ProtocolFamily::Craft,
		versions: vec![jet_protocol::ProtocolVersion { major: 1, minor: 8 }],
		capabilities: vec!["fork".into(), "runs".into()],
	};
	let negotiated = offer
		.negotiate(
			&craft.specification.protocol,
			jet_protocol::Negotiation::NewExecution,
		)
		.map_err(|_| unavailable())?;
	if negotiated.version.minor < 1
		|| !negotiated.capabilities.iter().any(|c| c == "runs")
	{
		return Err(unavailable());
	}
	let pin = PinnedCraft {
		id: id.into(),
		executable: craft.executable,
		sha256: craft.sha256,
		adapter_state: serde_json::to_string(&Contract {
			version: 1,
			boot_identity: filesystem::blocking(
				jet_runtime::execution_boot_identity,
			)
			.await?
			.map_err(|_| unavailable())?,
			craft_protocol: negotiated.version,
			helper_protocol: ProtocolVersion { major: 1, minor: 1 },
			specification: craft.specification,
		})
		.map_err(|_| unavailable())?,
	};
	pin.verify().await?;
	Ok(pin)
}

/// Chooses native delivery only from accepted, compatible Harness contracts.
/// Missing or incompatible source execution metadata deliberately falls back
/// to Core's provenance-marked context package.
pub(crate) async fn prepare_fork(
	mut plan: jet_core::LaunchPlan,
	source: Option<ForkLaunchSource>,
) -> Result<jet_core::LaunchPlan, CoreError> {
	let Some(source) = source else {
		return Ok(plan);
	};
	let destination = Contract::of(&plan.craft)?;
	let Ok(source_contract) = Contract::of(&source.craft) else {
		return Ok(plan);
	};
	let features = destination
		.specification
		.enabled_features()
		.map_err(|_| unavailable())?;
	let supports_fork = destination.craft_protocol.minor >= 4
		&& destination.specification.harness
			== source_contract.specification.harness
		&& features.iter().any(|feature| feature == "fork")
		&& destination
			.specification
			.protocol
			.capabilities
			.iter()
			.any(|capability| capability == "fork");
	let Some(identity) = source.native_conversation.filter(|identity| {
		!identity.is_empty()
			&& identity.len() <= 4096
			&& !identity.chars().any(char::is_control)
	}) else {
		return Ok(plan);
	};
	if supports_fork {
		let Some(fork) = plan.fork.as_mut() else {
			return Err(unavailable());
		};
		// ASVS 8.3.1: a declaration enables negotiation but grants no new file,
		// process, credential, or broker authority to the destination execution.
		fork.source_native_conversation = Some(identity);
	}
	Ok(plan)
}

/// A later Run selects the installed default; only active Runs retain old pins.
pub(crate) async fn prepare_next_run(
	home: &Path,
	mut plan: jet_core::LaunchPlan,
) -> Result<jet_core::LaunchPlan, CoreError> {
	let mut contract = Contract::of(&plan.craft)?;
	let current = load(home, &contract.specification.id).await?;
	if current.sha256 != plan.craft.sha256 {
		let updated = Contract::of(&current)?;
		if updated.specification.harness != contract.specification.harness {
			return Err(unavailable());
		}
		plan.craft = current;
		contract = updated;
	}
	plan.craft.id = contract.specification.id.clone();
	if plan.native_conversation.is_some()
		&& (!contract
			.specification
			.enabled_features()
			.map_err(|_| unavailable())?
			.iter()
			.any(|feature| feature == "resume")
			|| !contract
				.specification
				.protocol
				.capabilities
				.iter()
				.any(|capability| capability == "resume"))
	{
		return Err(unavailable());
	}
	contract.boot_identity =
		filesystem::blocking(jet_runtime::execution_boot_identity)
			.await?
			.map_err(|_| unavailable())?;
	plan.craft.adapter_state =
		serde_json::to_string(&contract).map_err(|_| unavailable())?;
	plan.craft.verify().await?;
	Ok(plan)
}

fn bounded_read(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
	let file = std::fs::File::open(path)?;
	if !file.metadata()?.is_file() {
		return Err(std::io::Error::other("not a file"));
	}
	let mut bytes = Vec::new();
	file.take(limit + 1).read_to_end(&mut bytes)?;
	if bytes.len() as u64 > limit {
		return Err(std::io::Error::other("too large"));
	}
	Ok(bytes)
}

fn unavailable() -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: "craft.unavailable".into(),
		retryable: false,
		message: "the accepted Craft is unavailable or incompatible".into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
