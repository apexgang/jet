//! `jetd`: the authoritative Jet daemon for one Plane (ADR-0003).
//!
//! `jetd` is a transport Adapter around `jet-core` (ADR-0047). It claims the
//! Plane's lifetime lock, opens the durable store, and serves the Jet
//! protocol over an owner-only local socket.
//!
//! Exit codes: `0` after a clean shutdown, `2` when another live `jetd`
//! already owns the Plane, `1` for any other failure.

mod artifact_stream;
mod child_control;
mod connection;
mod connection_pairing;
mod connection_session;
mod craft_processes;
mod craft_revocation;
mod craft_supervisor;
mod daemon;
mod execution_signal;
mod execution_termination;
mod extension_host;
mod installation_identity;
mod no_visa_broker;
mod remote_tool;
mod review_host;
mod run_craft;
mod run_host;
mod run_recovery;
mod stdio;
mod translate;
mod utility_host;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use jet_runtime::{InstallationChannel, JetHome};

#[derive(Parser)]
#[command(name = "jetd", version, about)]
struct Cli {
	#[command(subcommand)]
	subcommand: Subcommand,
}

/// What `jetd` was asked to do. Named apart from the domain's `Command`,
/// an authenticated state change.
#[derive(clap::Subcommand)]
enum Subcommand {
	/// Supervise one accepted Craft over a private lifetime pipe.
	#[command(hide = true)]
	CraftSupervisor {
		#[arg(long)]
		executable: PathBuf,
		#[arg(long)]
		socket: PathBuf,
		#[arg(long)]
		digest: String,
	},
	/// Execute one private, preauthorized bounded operation on standard I/O.
	#[command(hide = true)]
	RemoteWorker,
	/// Bridge SSH standard I/O to the running Plane's restricted handshake.
	Connect {
		/// Use standard input/output for the Jet protocol.
		#[arg(long, required = true)]
		stdio: bool,
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
	},
	/// Serve the Plane in the foreground until SIGTERM or SIGINT.
	Serve {
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
		/// How this daemon was installed and is managed.
		#[arg(long, value_enum, default_value_t = Channel::Development)]
		channel: Channel,
		/// Platform credential signing helper owned by the desktop installation.
		#[arg(long, requires = "identity_client_id")]
		identity_signer: Option<PathBuf>,
		/// Installation identity whose private key stays in platform storage.
		#[arg(long, requires = "identity_signer")]
		identity_client_id: Option<uuid::Uuid>,
		/// Trusted Jet release Ed25519 public key file, exactly 32 raw bytes.
		#[arg(long)]
		release_verification_key: Option<PathBuf>,
	},
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Channel {
	Development,
	Gui,
	Homebrew,
}

impl From<Channel> for InstallationChannel {
	fn from(channel: Channel) -> Self {
		match channel {
			Channel::Development => Self::Development,
			Channel::Gui => Self::Gui,
			Channel::Homebrew => Self::Homebrew,
		}
	}
}

#[tokio::main]
async fn main() -> ExitCode {
	let Cli { subcommand } = Cli::parse();
	match subcommand {
		Subcommand::CraftSupervisor {
			executable,
			socket,
			digest,
		} => match craft_supervisor::run(&executable, &socket, &digest).await {
			Ok(()) => ExitCode::SUCCESS,
			Err(error) => {
				eprintln!("jetd: Craft supervisor failed: {error}");
				ExitCode::FAILURE
			}
		},
		Subcommand::RemoteWorker => remote_tool::worker().await,
		Subcommand::Connect { home, .. } => {
			let Some(home) =
				home.map(JetHome::at).or_else(JetHome::for_current_user)
			else {
				return ExitCode::from(1);
			};
			let code = match stdio::connect(&home).await {
				Ok(()) => 0,
				Err(error) => {
					eprintln!("jetd: stdio connection failed: {error}");
					1
				}
			};
			// Tokio's blocking stdin read cannot be canceled at runtime shutdown.
			// This relay owns no state and has already flushed protocol output.
			std::process::exit(code)
		}
		Subcommand::Serve {
			home,
			channel,
			identity_signer,
			identity_client_id,
			release_verification_key,
		} => {
			let Some(home) =
				home.map(JetHome::at).or_else(JetHome::for_current_user)
			else {
				eprintln!("jetd: no --home given and HOME is not set");
				return ExitCode::from(1);
			};
			let release_key = match release_verification_key
				.as_deref()
				.map(craft_revocation::key)
				.transpose()
			{
				Ok(key) => key,
				Err(error) => {
					eprintln!(
						"jetd: cannot load release verification key: {error}"
					);
					return ExitCode::from(1);
				}
			};
			daemon::run(
				home,
				channel.into(),
				release_key,
				identity_signer.zip(identity_client_id).map(
					|(executable, client_id)| installation_identity::Identity {
						executable,
						client_id,
					},
				),
			)
			.await
		}
	}
}

mod terminal_host;
mod terminal_stream;

mod terminal_recovery;
