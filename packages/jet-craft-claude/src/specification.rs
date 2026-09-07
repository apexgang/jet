//! The accepted declaration this Craft speaks for, and the Harness it names.
use jet_craft_sdk::CraftError;
use jet_protocol::{CraftHostAccess, CraftSpecification};

/// The documented location of a Craft's declaration, read beside this
/// executable so the Harness path stays an installation decision.
const DECLARATION: &str = ".jet/craft-spec.toml";
/// The parser's own bound; refuse an oversized document before reading it.
const DECLARATION_BYTES: u64 = 64 * 1024;

/// Read the declaration installed beside this executable.
///
/// The host compares what the handshake carries against the document it
/// accepted, so an edited file here is refused rather than trusted: this read
/// chooses which declaration to present, never what it is allowed to do.
///
/// # Errors
/// Refuses a missing, oversized, or unusable declaration.
pub(crate) fn declaration() -> Result<CraftSpecification, CraftError> {
	let path = std::env::current_exe()
		.map_err(|_| CraftError::Incompatible)?
		.with_file_name(DECLARATION);
	// ASVS 2.2.1: bound the document before allocating it.
	if std::fs::metadata(&path)
		.map_err(|_| CraftError::Incompatible)?
		.len() > DECLARATION_BYTES
	{
		return Err(CraftError::Incompatible);
	}
	let text =
		std::fs::read_to_string(&path).map_err(|_| CraftError::Incompatible)?;
	jet_craft_sdk::parse_specification(&text)
}

/// The Harness named by the Craft's own accepted declaration. Reading it here
/// rather than hard-coding a path keeps this launch and the helper's accepted
/// executable disclosures from ever disagreeing.
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
