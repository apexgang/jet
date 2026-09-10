//! Run-role process supervision, independent from the daemon and Craft.
mod terminal;

use clap::Parser;
#[derive(Parser)]
#[command(name = "jetfueld", version)]
struct Cli {
	#[command(subcommand)]
	role: Role,
}
#[derive(clap::Subcommand)]
enum Role {
	/// Own one Workspace terminal PTY.
	Terminal {
		#[arg(long)]
		config: std::path::PathBuf,
	},
	/// Own a Harness process for one managed Run.
	Run {
		#[arg(long)]
		config: std::path::PathBuf,
	},
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
	let result = match Cli::parse().role {
		Role::Run { config } => execution::serve::serve(&config).await,
		Role::Terminal { config } => terminal::serve(&config).await,
	};
	match result {
		Ok(()) => std::process::ExitCode::SUCCESS,
		Err(_) => {
			eprintln!("jetfueld: execution supervision failed");
			std::process::ExitCode::FAILURE
		}
	}
}

mod execution;
