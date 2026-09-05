//! SSH relay to the one authoritative daemon, with no store or local login.

use jet_runtime::JetHome;
use tokio::io::{AsyncWriteExt, copy};
use tokio::net::UnixStream;

// Same length as the public preface; only this trusted relay prepends it.
pub(crate) const REMOTE_PREFACE: &[u8] = b"jet-remote-1\n";

pub(crate) async fn connect(home: &JetHome) -> std::io::Result<()> {
	let mut socket = UnixStream::connect(home.socket_path()).await?;
	// ASVS 8.3.3: the SSH peer never inherits this relay's local authority.
	socket.write_all(REMOTE_PREFACE).await?;
	let (mut read, mut write) = socket.into_split();
	let (mut input, mut output) = (tokio::io::stdin(), tokio::io::stdout());
	tokio::select! {
		result = copy(&mut input, &mut write) => { result?; }
		result = copy(&mut read, &mut output) => { result?; }
	}
	output.flush().await
}
