//! The accepted declaration this Craft speaks for, and its Harness program.
use jet_craft_sdk::CraftError;
use jet_protocol::{CraftHostAccess, CraftSpecification};

/// The declaration compiled into this executable. The digest a host pins for
/// the executable therefore covers it, wherever the executable is installed,
/// and the host still compares it with the one it accepted at handshake.
const DECLARATION: &str = include_str!("../.jet/craft-spec.toml");

/// The declaration this executable ships.
///
/// # Errors
/// Refuses only a declaration a broken build embedded.
pub(crate) fn declaration() -> Result<CraftSpecification, CraftError> {
	jet_craft_sdk::parse_specification(DECLARATION)
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

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	#[test]
	fn the_shipped_declaration_names_the_codex_harness() {
		let declaration = declaration().unwrap();

		assert_eq!(
			(
				declaration.id.as_str(),
				declaration.harness.as_str(),
				harness_program(&declaration).unwrap()
			),
			("codex", "codex", "codex".to_owned())
		);
	}
}
