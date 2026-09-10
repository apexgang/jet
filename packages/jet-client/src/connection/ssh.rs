//! System SSH endpoint authentication, independent of Jet Pairing.

use crate::{Client, ClientError, ClientIdentity};
use std::process::{Command, Stdio};

/// A system SSH destination (`[user@]host` or an SSH-config host alias).
/// Ports, identity files, and jump hosts are resolved by the user's SSH config.
#[derive(Debug, Clone)]
pub struct SshEndpoint(String);

impl SshEndpoint {
	/// Validates a destination as data, never command-line options or shell code.
	///
	/// # Errors
	/// Returns invalid input for an empty, oversized, or unsafe destination.
	pub fn new(destination: &str) -> std::io::Result<Self> {
		let components: Vec<_> = destination.split('@').collect();
		if destination.len() > 512
			|| components.len() > 2
			|| components
				.iter()
				.any(|part| part.is_empty() || part.starts_with('-'))
			|| !destination
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b"-._:@[]%".contains(&b))
		{
			return Err(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"invalid SSH destination",
			));
		}
		Ok(Self(destination.into()))
	}

	/// Builds the system SSH invocation. Unknown or changed keys must be
	/// resolved explicitly outside Jet before Pairing or remote login.
	pub fn command(&self) -> Command {
		let mut command = Command::new("ssh");
		// ASVS 1.2.5, 6.3.4: argument arrays and a fixed remote command.
		// Disable master reuse so every connection checks current host trust.
		command.args([
			"-T",
			"-a",
			"-S",
			"none",
			"-o",
			"StrictHostKeyChecking=yes",
			"-o",
			"VerifyHostKeyDNS=no",
			"-o",
			"BatchMode=yes",
			"-o",
			"ConnectTimeout=10",
			"-o",
			"ConnectionAttempts=1",
			"-o",
			"ClearAllForwardings=yes",
			"-o",
			"PermitLocalCommand=no",
			"-o",
			"RemoteCommand=none",
			"--",
			&self.0,
			"jetd",
			"connect",
			"--stdio",
		]);
		command
	}
}

impl Client {
	/// Launches the system SSH client and proves the Paired Client identity.
	/// The subprocess remains owned by this Client and exits on drop.
	///
	/// # Errors
	/// Returns endpoint/SSH failures separately from Jet handshake refusals.
	pub async fn connect_ssh(
		endpoint: &SshEndpoint,
		identity: &impl ClientIdentity,
	) -> Result<Self, ClientError> {
		let mut command = tokio::process::Command::from(endpoint.command());
		let mut child = command
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::inherit())
			.kill_on_drop(true)
			.spawn()?;
		let read = child.stdout.take().ok_or(ClientError::Closed)?;
		let write = child.stdin.take().ok_or(ClientError::Closed)?;
		let mut client = Self::connect_remote(read, write, identity).await?;
		client.ssh = Some(child);
		Ok(client)
	}
}
