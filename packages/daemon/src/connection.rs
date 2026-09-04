//! Serves one local Jet protocol connection: preface, handshake, requests,
//! drain.

use std::sync::Arc;
use std::time::Duration;

use jet_core::{Actor, ClientId, CommandEnvelope, CommandId, Core};
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ClientMessage, CommandRequest, ErrorCategory,
	Frame, FrameError, FrameLimits, FrameReader, FrameWriter, PREFACE,
	PROTOCOL_MINOR, PROTOCOL_VERSION, QueryRequest, RequestId, ServerHello,
	ServerMessage, WireError, decode_control, encode_control, raw_command,
};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;
use tokio::time::timeout;

use crate::translate;

/// How long a peer may take to complete the preface and handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

struct Connection {
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
}

/// Why no message could be received on the connection.
enum ReceiveError {
	/// The peer closed the connection or the transport failed.
	Disconnected,
	/// The peer violated the protocol; the reply explains how.
	Protocol(WireError),
}

/// Why a control message could not be sent: an encoding failure, which is
/// a programming error, or a transport failure, which means the peer is
/// gone. Callers close the connection either way.
type SendError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) async fn serve(
	core: Arc<Core>,
	stream: UnixStream,
	mut draining: watch::Receiver<bool>,
) {
	let Ok(Some((mut connection, actor))) =
		timeout(HANDSHAKE_TIMEOUT, open(stream)).await
	else {
		return;
	};
	connection
		.serve_requests(&core, &actor, &mut draining)
		.await;
}

async fn open(mut stream: UnixStream) -> Option<(Connection, Actor)> {
	let mut preface = vec![0u8; PREFACE.len()];
	if stream.read_exact(&mut preface).await.is_err() || preface != PREFACE {
		return None;
	}
	let (read, write) = stream.into_split();
	let mut connection = Connection {
		reader: FrameReader::new(read),
		writer: FrameWriter::new(write),
	};
	let actor = connection.handshake().await?;
	Some((connection, actor))
}

impl Connection {
	async fn handshake(&mut self) -> Option<Actor> {
		let hello: ClientHello = match self.receive().await {
			Ok(hello) => hello,
			Err(ReceiveError::Disconnected) => return None,
			Err(ReceiveError::Protocol(error)) => {
				let _ = self.send(&ServerHello::Rejected { error }).await;
				return None;
			}
		};
		let rejection = if hello.codec != CODEC_JSON_V1 {
			Some(wire_error(
				ErrorCategory::Incompatible,
				"protocol.unsupported_codec",
				format!("only the {CODEC_JSON_V1} codec is supported"),
			))
		} else if !hello.protocol.contains(PROTOCOL_VERSION) {
			Some(wire_error(
				ErrorCategory::Incompatible,
				"protocol.unsupported_version",
				format!(
					"only protocol version {PROTOCOL_VERSION} is supported"
				),
			))
		} else if hello.max_control_frame == 0 || hello.max_data_frame == 0 {
			Some(wire_error(
				ErrorCategory::InvalidInput,
				"protocol.invalid_frame_limits",
				"frame limits must be greater than zero".into(),
			))
		} else {
			None
		};
		if let Some(error) = rejection {
			let _ = self.send(&ServerHello::Rejected { error }).await;
			return None;
		}
		let limits = FrameLimits::default().negotiate(FrameLimits {
			control: hello.max_control_frame as usize,
			data: hello.max_data_frame as usize,
		});
		// Both peers send only the fields of the smaller minor (ADR-0019).
		#[expect(
			clippy::unnecessary_min_or_max,
			reason = "v1 has no minors yet; the negotiation stays general"
		)]
		let minor = hello.minor.min(PROTOCOL_MINOR);
		let welcome = ServerHello::Welcome {
			protocol: PROTOCOL_VERSION,
			minor,
			codec: CODEC_JSON_V1.into(),
			max_control_frame: frame_limit(limits.control),
			max_data_frame: frame_limit(limits.data),
			capabilities: vec![],
		};
		self.send(&welcome).await.ok()?;
		self.writer.set_limits(limits);
		Some(Actor::InteractiveClient {
			client_id: ClientId(hello.client_id),
		})
	}

	/// Answers requests until the peer leaves or the daemon drains. A request
	/// already received is answered before the drain is honored, so an
	/// accepted Command is never left without its reply (ADR-0088).
	async fn serve_requests(
		&mut self,
		core: &Core,
		actor: &Actor,
		draining: &mut watch::Receiver<bool>,
	) {
		loop {
			let payload = tokio::select! {
				biased;
				_ = draining.changed() => {
					let _ = self
						.send(&ServerMessage::Error {
							id: None,
							error: draining_error(),
						})
						.await;
					return;
				}
				received = self.receive_control() => match received {
					Ok(payload) => payload,
					Err(ReceiveError::Disconnected) => return,
					Err(ReceiveError::Protocol(error)) => {
						let _ = self
							.send(&ServerMessage::Error { id: None, error })
							.await;
						return;
					}
				},
			};
			let reply = match decode(&payload) {
				Ok(ClientMessage::Query { id, query }) => {
					answer(core, actor, id, &query)
				}
				Ok(ClientMessage::Command {
					id,
					command_id,
					command,
				}) => match raw_command(&payload) {
					Ok(raw) => execute(
						core,
						actor,
						id,
						command_id,
						&command,
						raw.get().as_bytes(),
					),
					Err(_) => ServerMessage::Error {
						id: Some(id),
						error: malformed(),
					},
				},
				Err(ReceiveError::Disconnected) => return,
				Err(ReceiveError::Protocol(error)) => {
					let _ = self
						.send(&ServerMessage::Error { id: None, error })
						.await;
					return;
				}
			};
			if self.send(&reply).await.is_err() {
				return;
			}
		}
	}

	async fn send<T: serde::Serialize>(
		&mut self,
		message: &T,
	) -> Result<(), SendError> {
		let payload = encode_control(message)?;
		self.writer.write(&Frame::Control(payload)).await?;
		Ok(())
	}

	/// Receives one decoded control message.
	async fn receive<T: serde::de::DeserializeOwned>(
		&mut self,
	) -> Result<T, ReceiveError> {
		let payload = self.receive_control().await?;
		decode(&payload)
	}

	/// Receives the payload of one control frame. Native decoder detail
	/// stays local so only stable, safe messages reach the peer (ADR-0068).
	async fn receive_control(&mut self) -> Result<Vec<u8>, ReceiveError> {
		match self.reader.read().await {
			Ok(Frame::Control(payload)) => Ok(payload),
			Ok(Frame::Data(_)) => Err(ReceiveError::Protocol(wire_error(
				ErrorCategory::InvalidInput,
				"protocol.unexpected_data_frame",
				"no data stream is open on this connection".into(),
			))),
			Err(FrameError::Oversized { .. } | FrameError::UnknownKind(_)) => {
				Err(ReceiveError::Protocol(wire_error(
					ErrorCategory::InvalidInput,
					"protocol.invalid_frame",
					"the frame violated the protocol limits".into(),
				)))
			}
			Err(FrameError::Closed | FrameError::Io(_)) => {
				Err(ReceiveError::Disconnected)
			}
		}
	}
}

fn decode<T: serde::de::DeserializeOwned>(
	payload: &[u8],
) -> Result<T, ReceiveError> {
	decode_control(payload).map_err(|_| ReceiveError::Protocol(malformed()))
}

fn answer(
	core: &Core,
	actor: &Actor,
	id: RequestId,
	query: &QueryRequest,
) -> ServerMessage {
	let result = blocking(|| core.query(actor, translate::query(query)))
		.and_then(translate::query_result);
	match result {
		Ok(result) => ServerMessage::QueryResult { id, result },
		Err(error) => ServerMessage::Error {
			id: Some(id),
			error: translate::error(error),
		},
	}
}

fn execute(
	core: &Core,
	actor: &Actor,
	id: RequestId,
	command_id: uuid::Uuid,
	command: &CommandRequest,
	request_bytes: &[u8],
) -> ServerMessage {
	let outcome = CommandEnvelope::new(
		CommandId(command_id),
		translate::command(command),
		request_bytes,
	)
	.and_then(|envelope| blocking(|| core.execute(actor, envelope)));
	match outcome {
		Ok(outcome) => ServerMessage::CommandResult {
			id,
			result: translate::command_outcome(outcome),
		},
		Err(error) => ServerMessage::Error {
			id: Some(id),
			error: translate::error(error),
		},
	}
}

/// Runs core work, which does synchronous SQLite I/O with full durability
/// (ADR-0057), off the async scheduler so accepting connections and
/// handling signals stay responsive.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
	tokio::task::block_in_place(work)
}

fn draining_error() -> WireError {
	WireError {
		category: ErrorCategory::Unavailable,
		code: "daemon.draining".into(),
		retryable: true,
		message: "jetd is shutting down; reconnect later and retry with the same Command identity".into(),
		revision_conflict: None,
		recovery_actions: vec![],
	}
}

fn malformed() -> WireError {
	wire_error(
		ErrorCategory::InvalidInput,
		"protocol.malformed",
		"the control frame is not a valid message".into(),
	)
}

fn wire_error(
	category: ErrorCategory,
	code: &str,
	message: String,
) -> WireError {
	WireError {
		category,
		code: code.into(),
		retryable: false,
		message,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}

fn frame_limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}
