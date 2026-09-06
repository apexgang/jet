//! Run-role process supervision, independent from the daemon and Craft.
mod native;
mod serve;
mod spool;

use clap::Parser;
#[derive(Parser)]
#[command(name = "jetfueld", version)]
struct Cli {
	#[command(subcommand)]
	role: Role,
}
#[derive(clap::Subcommand)]
enum Role {
	/// Own a Harness process for one managed Run.
	Run {
		#[arg(long)]
		config: std::path::PathBuf,
	},
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
	let Cli {
		role: Role::Run { config },
	} = Cli::parse();
	match serve::serve(&config).await {
		Ok(()) => std::process::ExitCode::SUCCESS,
		Err(_) => {
			eprintln!("jetfueld: execution supervision failed");
			std::process::ExitCode::FAILURE
		}
	}
}
