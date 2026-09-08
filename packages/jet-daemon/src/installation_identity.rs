//! Desktop-owned signing adapter. Jet receives signatures, never private keys.
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub(crate) struct Identity {
	pub(crate) executable: PathBuf,
	pub(crate) client_id: uuid::Uuid,
}
impl jet_client::ClientIdentity for Identity {
	fn client_id(&self) -> uuid::Uuid {
		self.client_id
	}
	async fn sign(&self, transcript: &[u8]) -> std::io::Result<[u8; 64]> {
		if !self.executable.is_absolute() || transcript.len() > 4096 {
			return Err(std::io::Error::other(
				"invalid installation signing configuration",
			));
		}
		// Only the startup configuration can choose this helper. It resolves the
		// installation's key from platform storage and writes exactly 64 bytes.
		let mut command = tokio::process::Command::new(&self.executable);
		command
			.arg("sign-connection-v1")
			.arg(self.client_id.to_string())
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::null());
		let mut operation = jet_runtime::NoVisaOperation::spawn(
			&mut command,
			tokio::time::sleep(Duration::from_secs(8)),
		)?;
		let mut stdin = operation.take_stdin().expect("piped signer input");
		let stdout = operation.take_stdout().expect("piped signer output");
		let output = tokio::time::timeout(Duration::from_secs(10), async {
			stdin.write_all(transcript).await?;
			drop(stdin);
			let mut output = Vec::new();
			stdout.take(65).read_to_end(&mut output).await?;
			Ok::<_, std::io::Error>(output)
		})
		.await;
		if !matches!(&output, Ok(Ok(bytes)) if bytes.len() == 64) {
			operation.stop();
		}
		let status = operation.wait().await?;
		let output = output.map_err(|_| {
			std::io::Error::other("installation signing timed out")
		})??;
		if !status.success() {
			return Err(std::io::Error::other("installation signing failed"));
		}
		output.try_into().map_err(|_| {
			std::io::Error::other("invalid installation signature length")
		})
	}
}
