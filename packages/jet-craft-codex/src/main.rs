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
	#[arg(long)]
	socket: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
	let arguments = Arguments::parse();
	let Ok(declaration) = specification::declaration() else {
		eprintln!("jet-craft-codex: shipped declaration is not usable");
		return std::process::ExitCode::FAILURE;
	};
	let Ok(listener) = tokio::net::UnixListener::bind(&arguments.socket) else {
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
