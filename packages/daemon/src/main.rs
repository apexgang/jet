//! `jetd`: the authoritative Jet daemon for one Plane (ADR-0003).
//!
//! `jetd` is a transport Adapter around `jet-core` (ADR-0047). It claims the
//! Plane's lifetime lock, opens the durable store, and serves the Jet
//! protocol over an owner-only local socket.
//!
//! Exit codes: `0` after a clean shutdown, `2` when another live `jetd`
//! already owns the Plane, `1` for any other failure.

mod connection;
mod daemon;
mod translate;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use jet_runtime::{InstallationChannel, JetHome};

#[derive(Parser)]
#[command(name = "jetd", version, about)]
struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	/// Run the daemon in the foreground until SIGTERM or SIGINT.
	Run {
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
		/// How this daemon was installed and is managed.
		#[arg(long, value_enum, default_value_t = Channel::Development)]
		channel: Channel,
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
	let Cli { command } = Cli::parse();
	match command {
		Command::Run { home, channel } => {
			let Some(home) =
				home.map(JetHome::at).or_else(JetHome::for_current_user)
			else {
				eprintln!("jetd: no --home given and HOME is not set");
				return ExitCode::from(1);
			};
			daemon::run(home, channel.into()).await
		}
	}
}
