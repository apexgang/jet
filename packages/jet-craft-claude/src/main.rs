//! Bundled Jet Craft for the Claude Code Harness (ADR-0046, ADR-0060).
//!
//! The host starts this executable with a private socket endpoint and serves
//! one execution connection per Run there (`docs/craft-protocol.md`). The
//! Run connection translates the Craft and Claude Code native protocols, asking
//! its helper to launch and feed the Harness. The separate Utility entrypoint
//! resolves transport authentication and performs one inference without a Harness.
mod approval;
mod execution;
mod extensions;
mod harness;
mod native;
mod presentation;
mod remote_tools;
mod specification;
mod usage;

use clap::Parser;

#[derive(Parser)]
#[command(version, about)]
struct Arguments {
	/// Execute one isolated Craft extension v1 operation.
	#[arg(long, conflicts_with_all = ["socket", "utility", "utility_model"])]
	extensions_v1: bool,
	/// Private, owner-only endpoint the host provisioned for this process.
	#[arg(long, required_unless_present_any = ["utility", "utility_model", "extensions_v1"])]
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
	if arguments.extensions_v1 {
		return if jet_craft_sdk::serve_extensions(extensions::handle)
			.await
			.is_ok()
		{
			std::process::ExitCode::SUCCESS
		} else {
			std::process::ExitCode::FAILURE
		};
	}
	if arguments.utility || arguments.utility_model {
		let result = if arguments.utility {
			jet_craft_sdk::serve_utility(
				jet_craft_sdk::UtilityProvider::Anthropic,
			)
			.await
		} else {
			jet_craft_sdk::utility_model(
				jet_craft_sdk::UtilityProvider::Anthropic,
			)
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
		eprintln!("jet-craft-claude: shipped declaration is not usable");
		return std::process::ExitCode::FAILURE;
	};
	let Ok(listener) = tokio::net::UnixListener::bind(&socket) else {
		eprintln!("jet-craft-claude: cannot serve the host's endpoint");
		return std::process::ExitCode::FAILURE;
	};
	// One process multiplexes the Runs sharing this accepted digest, and each
	// execution connection is independent of the others (ADR-0018).
	loop {
		let Ok((stream, _)) = listener.accept().await else {
			return std::process::ExitCode::FAILURE;
		};
		let declaration = declaration.clone();
		tokio::spawn(async move {
			if let Err(error) = execution::execution(stream, declaration).await
			{
				eprintln!("jet-craft-claude: execution ended: {error}");
			}
		});
	}
}
