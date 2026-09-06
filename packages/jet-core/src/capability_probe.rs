//! The Plane's own machine as a [`CapabilityProbe`] (ADR-0056, ADR-0086).
//!
//! Every external tool is invoked as an argument array without a shell, and
//! nothing here installs, updates, or configures a tool: the core only
//! reports what it found.

use std::future::Future;
use std::pin::Pin;

use tokio::process::Command;

use crate::capability::{
	CapabilityProbe, CredentialStoreKind, CredentialStoreStatus, ExternalTool,
	ExternalToolStatus, MAX_VERSION_CHARS, ObservedCapabilities, Platform,
	ToolAvailability,
};

/// Observes the machine this `jetd` runs on.
#[derive(Debug)]
pub(crate) struct SystemCapabilityProbe;

impl CapabilityProbe for SystemCapabilityProbe {
	fn observe(
		&self,
	) -> Pin<Box<dyn Future<Output = ObservedCapabilities> + Send + '_>> {
		Box::pin(async move {
			let mut external_tools =
				Vec::with_capacity(ExternalTool::ALL.len());
			for tool in ExternalTool::ALL {
				external_tools.push(ExternalToolStatus {
					tool,
					availability: detect(tool).await,
				});
			}
			ObservedCapabilities {
				platform: Platform {
					operating_system: std::env::consts::OS,
					architecture: std::env::consts::ARCH,
				},
				external_tools,
				credential_store: credential_store(),
				// Craft discovery and installation arrive with the Craft
				// issues; until then the Plane honestly reports none.
				crafts: Vec::new(),
			}
		})
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

/// Which credential store this platform resolves through, and whether it
/// can be reached. Jet never falls back to plaintext, so an unreachable
/// store is reported rather than replaced (ADR-0076).
///
/// This looks for the backend rather than into it. Telling a locked
/// backend from an open one means speaking the Secret Service D-Bus
/// interface on Linux and the Keychain API on macOS, which this Plane does
/// not yet do; until it does, a backend it can find is reported as
/// available, and a Plane reports [`CredentialStoreStatus::Locked`] only
/// through a probe that can see the difference.
fn credential_store() -> CredentialStoreStatus {
	if cfg!(target_os = "macos") {
		// The Keychain is part of the operating system.
		return CredentialStoreStatus::Available {
			kind: CredentialStoreKind::AppleKeychain,
		};
	}
	let kind = CredentialStoreKind::SecretService;
	// The Secret Service answers on the session bus. Its advertised address
	// or the socket the session manager left behind is the evidence that
	// one exists; a `jetd` started without a session has neither.
	let session_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
		|| std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|runtime| {
			std::path::Path::new(&runtime).join("bus").exists()
		});
	if session_bus {
		CredentialStoreStatus::Available { kind }
	} else {
		CredentialStoreStatus::Unavailable { kind }
	}
}

#[cfg(test)]
#[path = "capability_probe_tests.rs"]
mod tests;
