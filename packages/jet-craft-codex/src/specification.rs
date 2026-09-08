//! The accepted declaration this Craft speaks for, and its Harness program.
use jet_craft_sdk::CraftError;
use jet_protocol::{CraftHostAccess, CraftSpecification};

const DECLARATION: &str = ".jet/craft-spec.toml";
const DECLARATION_BYTES: u64 = 64 * 1024;

/// Read the bounded declaration installed beside this executable.
pub(crate) fn declaration() -> Result<CraftSpecification, CraftError> {
	let path = std::env::current_exe()
		.map_err(|_| CraftError::Incompatible)?
		.with_file_name(DECLARATION);
	// ASVS 2.2.1: reject an oversized untrusted declaration before reading it.
	if std::fs::metadata(&path)
		.map_err(|_| CraftError::Incompatible)?
		.len() > DECLARATION_BYTES
	{
		return Err(CraftError::Incompatible);
	}
	let text =
		std::fs::read_to_string(path).map_err(|_| CraftError::Incompatible)?;
	jet_craft_sdk::parse_specification(&text)
}

/// Resolve the executable from the declaration accepted by the host.
pub(crate) fn harness_program(
	specification: &CraftSpecification,
) -> Result<String, CraftError> {
	specification
		.host_access
		.iter()
		.find_map(|access| match access {
			CraftHostAccess::Executable { name } => Some(name.clone()),
			CraftHostAccess::Filesystem { .. }
			| CraftHostAccess::Environment { .. }
			| CraftHostAccess::Network { .. } => None,
		})
		.ok_or(CraftError::Incompatible)
}
