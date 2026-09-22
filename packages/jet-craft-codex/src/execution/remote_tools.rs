//! A native stdio MCP server relays one bounded call at a time to this Run.
//! Only jetd decides destination authority and executes remote operations.
use jet_craft_sdk::CraftError;
use jet_protocol::{CraftRemoteTool, NoVisaSelection, RemoteToolOutcome};
use serde_json::{Value, json};
use std::{
	io,
	os::unix::fs::{FileTypeExt, PermissionsExt},
	path::{Path, PathBuf},
	time::Duration,
};
use tokio::{
	io::{
		AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader,
	},
	net::{UnixListener, UnixStream},
	sync::{mpsc, oneshot},
};

const MESSAGE_BYTES: u64 = 1024 * 1024;

pub(crate) struct Call {
	pub request: CraftRemoteTool,
	pub response: oneshot::Sender<RemoteToolOutcome>,
}

pub(crate) struct RemoteTools {
	path: PathBuf,
	task: tokio::task::JoinHandle<()>,
	calls: mpsc::Receiver<Call>,
	pending: Option<(uuid::Uuid, oneshot::Sender<RemoteToolOutcome>)>,
}

impl RemoteTools {
	pub(crate) async fn bind(
		helper: &Path,
		selection: NoVisaSelection,
	) -> Result<Self, CraftError> {
		let path = helper.with_file_name("codex-mcp.sock");
		// The host supplies the owner-only helper directory. Remove only a
		// stale socket after a killed Craft, never a file or a live endpoint.
		if let Ok(metadata) = std::fs::symlink_metadata(&path) {
			if !metadata.file_type().is_socket() {
				return Err(CraftError::InvalidMessage);
			}
			match UnixStream::connect(&path).await {
				Err(error)
					if error.kind() == io::ErrorKind::ConnectionRefused =>
				{
					std::fs::remove_file(&path)
						.map_err(|_| CraftError::Disconnected)?;
				}
				_ => return Err(CraftError::InvalidMessage),
			}
		}
		let listener =
			UnixListener::bind(&path).map_err(|_| CraftError::Disconnected)?;
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
			.map_err(|_| CraftError::Disconnected)?;
		let (send, calls) = mpsc::channel(1);
		let task = tokio::spawn(async move {
			while let Ok((stream, _)) = listener.accept().await {
				let _ = serve(stream, &selection, &send).await;
			}
		});
		Ok(Self {
			path,
			task,
			calls,
			pending: None,
		})
	}

	pub(crate) fn pending(&self) -> bool {
		self.pending.is_some()
	}

	pub(crate) async fn next(&mut self) -> Result<CraftRemoteTool, CraftError> {
		let call = self.calls.recv().await.ok_or(CraftError::Disconnected)?;
		if self.pending.is_some() {
			return Err(CraftError::InvalidMessage);
		}
		self.pending = Some((call.request.operation_id, call.response));
		Ok(call.request)
	}

	pub(crate) fn reply(
		&mut self,
		operation_id: uuid::Uuid,
		outcome: RemoteToolOutcome,
	) -> Result<(), CraftError> {
		let (_, response) = self
			.pending
			.take_if(|(id, _)| *id == operation_id)
			.ok_or(CraftError::InvalidMessage)?;
		response.send(outcome).map_err(|_| CraftError::Disconnected)
	}

	pub(crate) fn configuration(&self) -> Result<Value, CraftError> {
		let executable =
			std::env::current_exe().map_err(|_| CraftError::Incompatible)?;
		Ok(json!({"mcp_servers.jet": {
			"command": executable, "args": ["--remote-tools-socket", self.path],
			"enabled": true, "required": true,
		}}))
	}
}

impl Drop for RemoteTools {
	fn drop(&mut self) {
		self.task.abort();
		let _ = std::fs::remove_file(&self.path);
	}
}

async fn serve(
	stream: UnixStream,
	selection: &NoVisaSelection,
	send: &mpsc::Sender<Call>,
) -> io::Result<()> {
	let (read, mut write) = stream.into_split();
	let request = tokio::time::timeout(
		Duration::from_secs(10),
		message(&mut BufReader::new(read)),
	)
	.await
	.map_err(io::Error::other)??;
	let Some(id) = request.get("id") else {
		return Ok(());
	};
	let result = match request["method"].as_str() {
		Some("initialize") => json!({
			"protocolVersion":"2024-11-05", "capabilities":{"tools":{}},
			"serverInfo":{"name":"jet","version":env!("CARGO_PKG_VERSION")},
		}),
		Some("ping") => json!({}),
		Some("tools/list") => json!({"tools":[{
			"name":"remote",
			"description":format!("Use bounded Jet tools on the selected destinations: {}. Native processes stay on the origin. Reuse operation_id only for the exact same action. Approval-required means no work ran; request destination review before retrying. Never retry a mutation with a new identity after an uncertain result.", serde_json::to_string(selection)?),
			"inputSchema":serde_json::from_str::<Value>(include_str!("../../../jet-protocol/contracts/remote-tool.schema.json"))?,
		}]}),
		Some("tools/call") if request["params"]["name"] == "remote" => {
			match serde_json::from_value::<CraftRemoteTool>(
				request["params"]["arguments"].clone(),
			) {
				Ok(request) => {
					let (response, result) = oneshot::channel();
					send.send(Call { request, response })
						.await
						.map_err(io::Error::other)?;
					let outcome = result.await.map_err(io::Error::other)?;
					json!({"isError":matches!(outcome, RemoteToolOutcome::Failed { .. }),
						"content":[{"type":"text","text":serde_json::to_string(&outcome)?}]})
				}
				Err(_) => {
					json!({"isError":true,"content":[{"type":"text","text":"Invalid bounded remote tool request"}]})
				}
			}
		}
		_ => {
			write
				.write_all(
					format!(
						"{}\n",
						rpc_error(id, -32601, "Unknown MCP method or tool")
					)
					.as_bytes(),
				)
				.await?;
			return Ok(());
		}
	};
	write
		.write_all(
			format!("{}\n", json!({"jsonrpc":"2.0","id":id,"result":result}))
				.as_bytes(),
		)
		.await
}

/// Native MCP subprocess entrypoint. Each request opens a fresh connection,
/// allowing later calls after Craft recovery. An interrupted call is never
/// replayed: its outcome might already have committed at the destination.
pub(crate) async fn stdio(path: &Path) -> io::Result<()> {
	let mut input = BufReader::new(tokio::io::stdin());
	let mut output = tokio::io::stdout();
	loop {
		let request = match message(&mut input).await {
			Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
				return Ok(());
			}
			other => other?,
		};
		let Some(id) = request.get("id") else {
			continue;
		};
		let response = match relay(path, &request).await {
			Ok(response) => response,
			Err(_) => rpc_error(
				id,
				-32603,
				"Remote tool outcome unknown: bridge disconnected. Do not retry a mutation with a new operation_id.",
			),
		};
		output.write_all(format!("{response}\n").as_bytes()).await?;
		output.flush().await?;
	}
}

async fn relay(path: &Path, request: &Value) -> io::Result<Value> {
	let mut stream = UnixStream::connect(path).await?;
	stream.write_all(format!("{request}\n").as_bytes()).await?;
	message(&mut BufReader::new(stream)).await
}

async fn message(
	reader: &mut (impl AsyncBufRead + Unpin),
) -> io::Result<Value> {
	let mut line = Vec::new();
	reader
		.take(MESSAGE_BYTES + 1)
		.read_until(b'\n', &mut line)
		.await?;
	if line.is_empty() {
		return Err(io::ErrorKind::UnexpectedEof.into());
	}
	if line.len() as u64 > MESSAGE_BYTES || line.last() != Some(&b'\n') {
		return Err(io::ErrorKind::InvalidData.into());
	}
	serde_json::from_slice(&line).map_err(io::Error::other)
}

fn rpc_error(id: &Value, code: i32, message: &str) -> Value {
	json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
