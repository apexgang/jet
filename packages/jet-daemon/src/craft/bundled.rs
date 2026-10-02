//! The Bundled Crafts of the release this `jetd` belongs to (ADR-0107).
//!
//! A release payload pins every executable it ships in the `manifest.json`
//! beside `jetd`, in both the GUI-managed `~/.jet/core` layout and the
//! Homebrew keg. Each Bundled Craft embeds its Craft specification, and the
//! same documents are compiled in here, so what `jetd` registers is what the
//! Craft presents at its handshake.

use crate::installation::manifest::ReleaseManifest;
use jet_core::{
	BundledCraft, BundledOutcome, BundledRegistration, CapabilitySnapshot,
	CraftId,
};
use jet_runtime::{Diagnostic, DiagnosticComponent};
use std::path::Path;

/// Each Bundled Craft: its executable in the payload, its Craft id, and the
/// Craft specification it embeds.
const CRAFTS: [(&str, &str, &str); 2] = [
	(
		"jet-craft-claude",
		"claude-code",
		include_str!("../../../jet-craft-claude/.jet/craft-spec.toml"),
	),
	(
		"jet-craft-codex",
		"codex",
		include_str!("../../../jet-craft-codex/.jet/craft-spec.toml"),
	),
];

/// The Bundled Crafts of the running release, or none for a `jetd` that was
/// not installed from a release payload of its own version, such as a
/// development build.
pub(crate) fn release() -> Vec<BundledCraft> {
	// Canonical, so a Homebrew `bin` symlink or the `current` link resolves
	// to the directory that holds the manifest, once for this start.
	let Some(directory) = std::env::current_exe()
		.and_then(std::fs::canonicalize)
		.ok()
		.and_then(|executable| executable.parent().map(Path::to_path_buf))
	else {
		return Vec::new();
	};
	release_in(&directory, env!("CARGO_PKG_VERSION"))
}

fn release_in(directory: &Path, version: &str) -> Vec<BundledCraft> {
	let manifest = match ReleaseManifest::read(directory) {
		Ok(manifest) => manifest,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Vec::new();
		}
		Err(error) => {
			Diagnostic::warn(
				DiagnosticComponent::Craft,
				"the release manifest beside jetd is not usable",
			)
			.failure(&error)
			.emit();
			return Vec::new();
		}
	};
	if manifest.version != version {
		Diagnostic::warn(
			DiagnosticComponent::Craft,
			"the release manifest beside jetd is for another version",
		)
		.identity("version", &manifest.version)
		.emit();
		return Vec::new();
	}
	CRAFTS
		.iter()
		.filter_map(|(executable, _, specification)| {
			Some(BundledCraft {
				executable: directory.join(executable),
				sha256: manifest.executables.get(*executable)?.clone(),
				specification,
				version: manifest.version.clone(),
			})
		})
		.collect()
}

/// Reports what registration did, one Diagnostic per Bundled Craft that
/// did not simply stay registered.
pub(crate) fn report(registration: &BundledRegistration) {
	let outcomes = match registration {
		BundledRegistration::Deferred => {
			Diagnostic::warn(
				DiagnosticComponent::Craft,
				"the bundled Crafts are not registered while the Security \
				 audit is not trusted",
			)
			.emit();
			return;
		}
		BundledRegistration::Done(outcomes) => outcomes,
	};
	for (id, outcome) in outcomes {
		let diagnostic = match outcome {
			BundledOutcome::Unchanged => continue,
			BundledOutcome::Registered => Diagnostic::info(
				DiagnosticComponent::Craft,
				"registered a bundled Craft",
			),
			BundledOutcome::KeptInstalled => Diagnostic::info(
				DiagnosticComponent::Craft,
				"an installed Craft keeps the id of a bundled Craft",
			),
			BundledOutcome::KeptUnreadable => Diagnostic::warn(
				DiagnosticComponent::Craft,
				"an unreadable Craft manifest blocks a bundled Craft; delete \
				 it to restore the bundled Craft",
			),
			BundledOutcome::Revoked => Diagnostic::warn(
				DiagnosticComponent::Craft,
				"the bundled Craft's digest is revoked",
			),
			BundledOutcome::Failed(error) => Diagnostic::warn(
				DiagnosticComponent::Craft,
				"cannot register a bundled Craft",
			)
			.code(&error.code)
			.failure(error),
		};
		diagnostic.identity("craft", id).emit();
	}
}

/// Warns about a Bundled Craft the Capability snapshot does not list,
/// which registration alone cannot tell: a damaged Artifact of the right
/// size is only found when the snapshot verifies it.
pub(crate) fn check_listed(capabilities: &CapabilitySnapshot) {
	for (_, id, _) in CRAFTS {
		if !capabilities
			.crafts
			.iter()
			.any(|craft| craft.craft == CraftId(id.into()))
		{
			Diagnostic::warn(
				DiagnosticComponent::Craft,
				"a bundled Craft is not an installed Craft",
			)
			.identity("craft", &id)
			.emit();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use jet_protocol::CraftHostAccess;
	use pretty_assertions::assert_eq;

	/// What the Utility and review hosts route by and require of each.
	#[test]
	fn each_bundled_declaration_has_the_identity_and_access_jetd_routes_by() {
		let declared: Vec<_> = CRAFTS
			.iter()
			.map(|(executable, id, specification)| {
				let specification =
					jet_craft_sdk::parse_specification(specification).unwrap();
				let reaches = |access: CraftHostAccess| {
					specification.host_access.contains(&access)
				};
				(
					*executable,
					*id,
					specification.id.clone(),
					reaches(CraftHostAccess::Executable {
						name: "curl".into(),
					}),
					reaches(CraftHostAccess::Network {
						destination: "api.anthropic.com".into(),
					}) || reaches(CraftHostAccess::Network {
						destination: "api.openai.com".into(),
					}),
				)
			})
			.collect();

		assert_eq!(
			declared,
			vec![
				(
					"jet-craft-claude",
					"claude-code",
					"claude-code".to_owned(),
					true,
					true
				),
				("jet-craft-codex", "codex", "codex".to_owned(), true, true),
			]
		);
	}

	fn write_manifest(directory: &Path, version: &str) {
		let manifest = serde_json::json!({
			"version": version,
			"target": "x86_64-unknown-linux-gnu",
			"protocol_major": 1,
			"protocol_minor": 0,
			"schema_version": 1,
			"executables": {
				"jetd": "a".repeat(64),
				"jetfueld": "b".repeat(64),
				"jet-craft-claude": "c".repeat(64),
				"jet-craft-codex": "d".repeat(64),
			},
		});
		std::fs::write(directory.join("manifest.json"), manifest.to_string())
			.unwrap();
	}

	#[test]
	fn the_release_manifest_of_this_version_names_both_bundled_crafts() {
		let directory = tempfile::tempdir().unwrap();
		write_manifest(directory.path(), "1.2.3");

		assert_eq!(
			release_in(directory.path(), "1.2.3"),
			vec![
				BundledCraft {
					executable: directory.path().join("jet-craft-claude"),
					sha256: "c".repeat(64),
					specification: CRAFTS[0].2,
					version: "1.2.3".into(),
				},
				BundledCraft {
					executable: directory.path().join("jet-craft-codex"),
					sha256: "d".repeat(64),
					specification: CRAFTS[1].2,
					version: "1.2.3".into(),
				},
			]
		);
	}

	#[test]
	fn no_release_or_another_version_registers_nothing() {
		let directory = tempfile::tempdir().unwrap();
		assert_eq!(release_in(directory.path(), "1.2.3"), Vec::new());

		write_manifest(directory.path(), "1.2.2");
		assert_eq!(release_in(directory.path(), "1.2.3"), Vec::new());

		std::fs::write(directory.path().join("manifest.json"), "{").unwrap();
		assert_eq!(release_in(directory.path(), "1.2.3"), Vec::new());
	}
}
