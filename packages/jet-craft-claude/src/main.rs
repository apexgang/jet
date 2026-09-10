//! Bundled Jet Craft for the Claude Code Harness (ADR-0046, ADR-0060).
//!
//! The host starts this executable with a private socket endpoint and serves
//! one execution connection per Run there (`docs/craft-protocol.md`). The
//! Run connection translates the Craft and Claude Code native protocols, asking
//! its helper to launch and feed the Harness. The separate Utility and review
//! entrypoints resolve transport authentication and perform one inference
//! each, without a Harness and without reaching any Run.
mod execution;
mod extensions;
mod specification;
mod usage;

use clap::Parser;
use jet_craft_sdk::OneShot;

/// The Provider this Craft's Harness authenticates through.
const PROVIDER: jet_craft_sdk::UtilityProvider =
	jet_craft_sdk::UtilityProvider::Anthropic;

#[derive(Parser)]
#[command(version, about)]
struct Arguments {
	/// Execute one isolated Craft extension v1 operation.
	#[arg(long, conflicts_with_all = ["socket", "utility", "utility_model", "review", "review_model"])]
	extensions_v1: bool,
	/// Private, owner-only endpoint the host provisioned for this process.
	#[arg(long, required_unless_present_any = jet_craft_sdk::ONE_SHOT_FLAGS)]
	socket: Option<std::path::PathBuf>,
	/// Execute exactly one isolated Utility v1 request on stdin/stdout.
	#[arg(long, conflicts_with_all = ["socket", "utility_model", "review", "review_model", "extensions_v1"])]
	utility: bool,
	/// Report the Utility v1 Model selection without receiving content.
	#[arg(long, conflicts_with_all = ["socket", "utility", "review", "review_model", "extensions_v1"])]
	utility_model: bool,
	/// Review exactly one held approval request on stdin/stdout.
	#[arg(long, conflicts_with_all = ["socket", "utility", "utility_model", "review_model", "extensions_v1"])]
	review: bool,
	/// Report the reviewer selection without receiving content.
	#[arg(long, conflicts_with_all = ["socket", "utility", "utility_model", "review", "extensions_v1"])]
	review_model: bool,
}

impl Arguments {
	/// The isolated entrypoint this invocation selected, if any. Clap has
	/// already refused every combination but one.
	fn one_shot(&self) -> Option<OneShot> {
		[
			(self.utility, OneShot::Utility),
			(self.utility_model, OneShot::UtilityModel),
			(self.review, OneShot::Review),
			(self.review_model, OneShot::ReviewModel),
		]
		.into_iter()
		.find_map(|(selected, one_shot)| selected.then_some(one_shot))
	}
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
	if let Some(one_shot) = arguments.one_shot() {
		return if one_shot.serve(PROVIDER).await.is_ok() {
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
