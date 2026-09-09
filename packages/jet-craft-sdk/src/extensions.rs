//! Bounded one-shot extension transport; native semantics belong to each Craft.
use jet_protocol::{CraftExtensionReply, CraftExtensionRequest};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Handle one owner-only extension request and exit without creating a Run.
/// # Errors
/// Returns a bounded transport or native adapter failure without native diagnostics.
pub async fn serve_extensions<F, Fut>(handle: F) -> io::Result<()>
where
	F: FnOnce(CraftExtensionRequest) -> Fut,
	Fut: std::future::Future<Output = io::Result<CraftExtensionReply>>,
{
	let mut bytes = Vec::new();
	tokio::io::stdin()
		.take(131073)
		.read_to_end(&mut bytes)
		.await?;
	if bytes.len() > 131072 {
		return Err(extension_error());
	}
	let request =
		jet_protocol::decode_control(&bytes).map_err(|_| extension_error())?;
	let reply = handle(request).await?;
	let bytes = serde_json::to_vec(&reply).map_err(|_| extension_error())?;
	if bytes.len() > 131072 {
		return Err(extension_error());
	}
	let mut stdout = tokio::io::stdout();
	stdout.write_all(&bytes).await?;
	// Tokio stdout buffers blocking writes; drain them before the process exits.
	stdout.flush().await
}
/// Content-free failure for native extension operations.
#[must_use]
pub fn extension_error() -> io::Error {
	io::Error::other("native extension operation unavailable")
}
