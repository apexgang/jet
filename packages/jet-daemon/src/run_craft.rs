//! Owner-provisioned Craft declarations translated into an opaque execution pin.
use crate::run_host::filesystem;
use jet_core::{CoreError, PinnedCraft};
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
			|| contract.craft_protocol
				!= (ProtocolVersion { major: 1, minor: 1 })
			|| contract.helper_protocol
				!= (ProtocolVersion { major: 1, minor: 0 })
		{
			return Err(unavailable());
		}
		Ok(contract)
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
		versions: vec![jet_protocol::ProtocolVersion { major: 1, minor: 1 }],
		capabilities: vec!["runs".into()],
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
		executable: craft.executable,
		sha256: craft.sha256,
		adapter_state: serde_json::to_string(&Contract {
			version: 1,
			craft_protocol: ProtocolVersion { major: 1, minor: 1 },
			helper_protocol: ProtocolVersion { major: 1, minor: 0 },
			specification: craft.specification,
		})
		.map_err(|_| unavailable())?,
	};
	pin.verify().await?;
	Ok(pin)
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
