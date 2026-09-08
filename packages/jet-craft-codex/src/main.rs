//! Bundled Jet Craft for the Codex Harness (ADR-0046, ADR-0060).
mod approval;
mod execution;
mod harness;
mod presentation;
mod specification;

use clap::Parser;

#[derive(Parser)]
#[command(version, about)]
struct Arguments {
	/// Private, owner-only endpoint provisioned by the host.
	#[arg(long, required_unless_present_any = ["utility", "utility_model"])]
	socket: Option<std::path::PathBuf>,
	/// Execute exactly one isolated Utility v1 request on stdin/stdout.
	#[arg(long, conflicts_with_all = ["socket", "utility_model"])]
	utility: bool,
	/// Report the Utility v1 Model selection without receiving content.
	#[arg(long, conflicts_with_all = ["socket", "utility"])]
	utility_model: bool,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
	let arguments = Arguments::parse();
	if arguments.utility || arguments.utility_model {
		let result = if arguments.utility {
			jet_craft_sdk::serve_utility(jet_craft_sdk::UtilityProvider::OpenAi)
				.await
		} else {
			jet_craft_sdk::utility_model(jet_craft_sdk::UtilityProvider::OpenAi)
				.await
		};
		return if result.is_ok() {
			std::process::ExitCode::SUCCESS
		} else {
			std::process::ExitCode::FAILURE
		};
	}
	let Some(socket) = arguments.socket else {
		return std::process::ExitCode::FAILURE;
	};
	let Ok(declaration) = specification::declaration() else {
		eprintln!("jet-craft-codex: shipped declaration is not usable");
		return std::process::ExitCode::FAILURE;
	};
	let Ok(listener) = tokio::net::UnixListener::bind(&socket) else {
		eprintln!("jet-craft-codex: cannot serve the host's endpoint");
		return std::process::ExitCode::FAILURE;
	};
	loop {
		let Ok((stream, _)) = listener.accept().await else {
			return std::process::ExitCode::FAILURE;
		};
		let declaration = declaration.clone();
		tokio::spawn(async move {
			if let Err(error) = execution::execution(stream, declaration).await
			{
				eprintln!("jet-craft-codex: execution ended: {error}");
			}
		});
	}
}
