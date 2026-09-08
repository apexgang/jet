//! Fixed HTTPS endpoints and bounded subprocess transport; secrets use stdin.
use crate::{
	CraftError,
	utility::{UtilityProvider, invalid},
};
use std::process::Stdio;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	process::Command,
};

pub(crate) async fn request(
	provider: UtilityProvider,
	key: &str,
	body: &serde_json::Value,
) -> Result<Vec<u8>, CraftError> {
	let (url, authorization) = match provider {
		UtilityProvider::OpenAi => (
			"https://api.openai.com/v1/responses",
			format!("Authorization: Bearer {key}"),
		),
		UtilityProvider::Anthropic => (
			"https://api.anthropic.com/v1/messages",
			format!("x-api-key: {key}"),
		),
	};
	let config = format!(
		"url = {}\nheader = {}\nheader = {}\nheader = {}\ndata-binary = {}\n",
		quoted(url),
		quoted(&authorization),
		quoted("Content-Type: application/json"),
		quoted("anthropic-version: 2023-06-01"),
		quoted(&body.to_string())
	);
	let mut command = Command::new("curl");
	// Disable curlrc, redirects, retries, proxy routing and diagnostics. The
	// model cannot select a URL or invoke any other transport operation.
	command.args([
		"--disable",
		"--silent",
		"--fail",
		"--proto",
		"=https",
		"--proxy",
		"",
		"--max-time",
		"20",
		"--max-filesize",
		"65536",
		"--config",
		"-",
	]);
	run(&mut command, config.as_bytes(), 65536).await
}
fn quoted(value: &str) -> String {
	format!(
		"\"{}\"",
		value
			.replace('\\', "\\\\")
			.replace('"', "\\\"")
			.replace('\n', "\\n")
			.replace('\r', "\\r")
	)
}
pub(crate) async fn run(
	command: &mut Command,
	input: &[u8],
	limit: u64,
) -> Result<Vec<u8>, CraftError> {
	let mut child = command
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true)
		.spawn()
		.map_err(|_| invalid())?;
	let mut stdin = child.stdin.take().ok_or_else(invalid)?;
	let stdout = child.stdout.take().ok_or_else(invalid)?;
	let write = async {
		stdin.write_all(input).await.map_err(|_| invalid())?;
		drop(stdin);
		Ok::<_, CraftError>(())
	};
	let read = async {
		let mut bytes = Vec::new();
		stdout
			.take(limit + 1)
			.read_to_end(&mut bytes)
			.await
			.map_err(|_| invalid())?;
		if bytes.len() as u64 > limit {
			return Err(invalid());
		}
		Ok(bytes)
	};
	let (_, bytes) = tokio::try_join!(write, read)?;
	if !child.wait().await.map_err(|_| invalid())?.success() {
		return Err(invalid());
	}
	Ok(bytes)
}
