//! The accepted declaration this Craft speaks for, and the Harness it names.
use jet_craft_sdk::CraftError;
use jet_protocol::{CraftHostAccess, CraftSpecification};

/// The declaration compiled into this executable. The digest a host pins for
/// the executable therefore covers it, wherever the executable is installed:
/// a declaration beside the executable would be shared by every Craft in one
/// directory, including the content-addressed Artifacts.
const DECLARATION: &str = include_str!("../.jet/craft-spec.toml");

/// The declaration this executable ships.
///
/// The host compares what the handshake carries against the document it
/// accepted, so this chooses which declaration to present, never what it is
/// allowed to do.
///
/// # Errors
/// Refuses only a declaration a broken build embedded.
pub(crate) fn declaration() -> Result<CraftSpecification, CraftError> {
	jet_craft_sdk::parse_specification(DECLARATION)
}

/// The Harness named by the Craft's own accepted declaration. Reading it here
/// rather than hard-coding a name keeps this launch and the helper's accepted
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

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn the_shipped_declaration_names_the_claude_code_harness() {
		let declaration = declaration().unwrap();

		assert_eq!(
			(
				declaration.id.as_str(),
				declaration.harness.as_str(),
				harness_program(&declaration).unwrap()
			),
			("claude-code", "claude-code", "claude".to_owned())
		);
	}
}
