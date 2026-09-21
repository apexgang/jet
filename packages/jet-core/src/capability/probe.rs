//! The Plane's own machine as a [`CapabilityProbe`] (ADR-0056, ADR-0086).
//!
//! Every external tool is invoked as an argument array without a shell, and
//! nothing here installs, updates, or configures a tool: the core only
//! reports what it found.

use crate::capability::{
	CapabilityProbe, CredentialStoreStatus, CredentialStoreVerification,
	ExternalTool, ExternalToolStatus, MAX_VERSION_CHARS, ObservedCapabilities,
	Platform, ToolAvailability, credential_store,
};
use std::{future::Future, pin::Pin};
use tokio::process::Command;

/// Observes the machine this `jetd` runs on.
#[derive(Debug)]
pub(crate) struct SystemCapabilityProbe;

impl CapabilityProbe for SystemCapabilityProbe {
	fn power(
		&self,
	) -> Pin<Box<dyn Future<Output = jet_runtime::PowerState> + Send + '_>> {
		Box::pin(jet_runtime::observe_power())
	}

	fn observe(
		&self,
	) -> Pin<Box<dyn Future<Output = ObservedCapabilities> + Send + '_>> {
		Box::pin(async move {
			// Every tool is asked at once; the snapshot still lists them in
			// the order of `ExternalTool::ALL`. The ready line waits for
			// this, and four version commands in a row cost it more than
			// one (ADR-0022).
			let detections: Vec<_> = ExternalTool::ALL
				.into_iter()
				.map(|tool| tokio::spawn(detect(tool)))
				.collect();
			// The credential store is asked at the same time, for the same
			// reason; a store that does not answer is unavailable, so a
			// probe that panicked reports what a silent one does.
			let credential_store = tokio::spawn(credential_store::observe());
			let mut external_tools =
				Vec::with_capacity(ExternalTool::ALL.len());
			for (tool, detection) in
				ExternalTool::ALL.into_iter().zip(detections)
			{
				external_tools.push(ExternalToolStatus {
					tool,
					// A detection that panicked observed nothing, which
					// is what a missing tool reports too.
					availability: detection
						.await
						.unwrap_or(ToolAvailability::Missing),
				});
			}
			ObservedCapabilities {
				power: jet_runtime::observe_power().await,
				platform: Platform {
					operating_system: std::env::consts::OS,
					architecture: std::env::consts::ARCH,
				},
				external_tools,
				credential_store: credential_store.await.unwrap_or(
					CredentialStoreStatus::Unavailable {
						kind: credential_store::KIND,
					},
				),
				// Craft discovery and installation arrive with the Craft
				// issues; until then the Plane honestly reports none.
				crafts: Vec::new(),
			}
		})
	}

	fn verify_credential_store(
		&self,
	) -> Pin<Box<dyn Future<Output = CredentialStoreVerification> + Send + '_>>
	{
		Box::pin(credential_store::verify())
	}
}

/// Runs one tool's version command and keeps the line it answered with.
async fn detect(tool: ExternalTool) -> ToolAvailability {
	run_version(tool.as_str(), version_arguments(tool)).await
}

/// Asks one program for its version and reads the line it answers with.
async fn run_version(program: &str, arguments: &[&str]) -> ToolAvailability {
	// ASVS 1.2.5 and 5.3.8: the program and its arguments are passed as an
	// array, so nothing a tool or its environment contains is interpreted
	// as shell source (ADR-0056).
	let output = Command::new(program)
		.args(arguments)
		.kill_on_drop(true)
		.output()
		.await;
	let Ok(output) = output else {
		return ToolAvailability::Missing;
	};
	// `ssh -V` writes its version to standard error, and a tool may report
	// its version with a nonzero status, so the first line either stream
	// carries is the evidence rather than the exit code.
	first_line(&output.stdout)
		.or_else(|| first_line(&output.stderr))
		.map_or(ToolAvailability::Missing, |version| {
			ToolAvailability::Present { version }
		})
}

fn version_arguments(tool: ExternalTool) -> &'static [&'static str] {
	match tool {
		ExternalTool::Git => &["--version"],
		ExternalTool::GitLfs => &["version"],
		ExternalTool::Ssh => &["-V"],
		ExternalTool::Tailscale => &["version"],
	}
}

/// The first non-empty line of a tool's output, bounded for the snapshot.
fn first_line(output: &[u8]) -> Option<String> {
	let text = String::from_utf8_lossy(output);
	let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
	Some(line.chars().take(MAX_VERSION_CHARS).collect())
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;

	use super::{first_line, run_version};
	use crate::capability::ToolAvailability;

	/// A tool reports its version through whichever stream it prefers, and a
	/// version line the Plane cannot bound would grow every snapshot that
	/// carries it.
	#[test]
	fn a_version_line_is_the_first_non_empty_line_and_is_bounded() {
		let git = first_line(b"git version 2.51.0\n");
		let padded = first_line(b"\n   \nOpenSSH_9.9p2, OpenSSL 3.5.4\nmore\n");
		let talkative = first_line(&vec![b'x'; 1_000]);
		let silent = first_line(b"   \n\n");

		assert_eq!(
			(git, padded, talkative.map(|line| line.len()), silent),
			(
				Some("git version 2.51.0".into()),
				Some("OpenSSH_9.9p2, OpenSSL 3.5.4".into()),
				Some(120),
				None
			)
		);
	}

	/// A Plane that does not have a tool must report it as missing rather than
	/// failing to observe itself at all.
	#[tokio::test]
	async fn a_program_that_cannot_be_run_is_missing() {
		let absent =
			run_version("jet-tool-no-plane-installs", &["--version"]).await;

		assert_eq!(absent, ToolAvailability::Missing);
	}
}
