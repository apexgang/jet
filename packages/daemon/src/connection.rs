//! Serves one local Jet protocol connection: preface, handshake, requests.

use std::sync::Arc;
use std::time::Duration;

use jet_core::{Actor, ClientId, CommandEnvelope, CommandId, Core};
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ErrorCategory, Frame, FrameError, FrameLimits,
	FrameReader, FrameWriter, PREFACE, PROTOCOL_VERSION, QueryRequest,
	RequestId, ServerHello, ServerMessage, WireError, decode_control,
	encode_control,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
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

#[derive(Deserialize)]
struct IncomingClientMessage {
	kind: IncomingMessageKind,
	id: RequestId,
	#[serde(default)]
	query: Option<QueryRequest>,
	#[serde(default)]
	command_id: Option<uuid::Uuid>,
	#[serde(default)]
	command: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum IncomingMessageKind {
	Query,
	Command,
}

pub(crate) async fn serve(core: Arc<Core>, stream: UnixStream) {
	let Ok(Some((mut connection, actor))) =
		timeout(HANDSHAKE_TIMEOUT, open(stream)).await
	else {
		return;
	};
	connection.serve_requests(&core, &actor).await;
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
		let welcome = ServerHello::Welcome {
			protocol: PROTOCOL_VERSION,
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

	async fn serve_requests(&mut self, core: &Core, actor: &Actor) {
		loop {
			let message: IncomingClientMessage = match self.receive().await {
				Ok(message) => message,
				Err(ReceiveError::Disconnected) => return,
				Err(ReceiveError::Protocol(error)) => {
					let _ = self
						.send(&ServerMessage::Error { id: None, error })
						.await;
					return;
				}
			};
			let reply = match message {
				IncomingClientMessage {
					kind: IncomingMessageKind::Query,
					id,
					query: Some(query),
					command_id: None,
					command: None,
				} => answer(core, actor, id, &query),
				IncomingClientMessage {
					kind: IncomingMessageKind::Command,
					id,
					query: None,
					command_id: Some(command_id),
					command: Some(command),
				} => match decode_control(command.get().as_bytes()) {
					Ok(decoded) => execute(
						core,
						actor,
						id,
						command_id,
						&decoded,
						command.get().as_bytes(),
					),
					Err(_) => ServerMessage::Error {
						id: Some(id),
						error: wire_error(
							ErrorCategory::InvalidInput,
							"protocol.malformed",
							"the control frame is not a valid message".into(),
						),
					},
				},
				IncomingClientMessage { id, .. } => ServerMessage::Error {
					id: Some(id),
					error: wire_error(
						ErrorCategory::InvalidInput,
						"protocol.malformed",
						"the control frame is not a valid message".into(),
					),
				},
			};
			if self.send(&reply).await.is_err() {
				return;
			}
		}
	}

	async fn send<T: serde::Serialize>(
		&mut self,
		message: &T,
	) -> Result<(), ()> {
		let payload = encode_control(message).map_err(|_| ())?;
		self.writer
			.write(&Frame::Control(payload))
			.await
			.map_err(|_| ())
	}

	/// Receives one control message. Native decoder detail stays local so
	/// only stable, safe messages reach the peer (ADR-0068).
	async fn receive<T: serde::de::DeserializeOwned>(
		&mut self,
	) -> Result<T, ReceiveError> {
		let invalid = |code: &str, message: &str| {
			ReceiveError::Protocol(wire_error(
				ErrorCategory::InvalidInput,
				code,
				message.into(),
			))
		};
		match self.reader.read().await {
			Ok(Frame::Control(payload)) => {
				decode_control(&payload).map_err(|_| {
					invalid(
						"protocol.malformed",
						"the control frame is not a valid message",
					)
				})
			}
			Ok(Frame::Data(_)) => Err(invalid(
				"protocol.unexpected_data_frame",
				"no data stream is open on this connection",
			)),
			Err(FrameError::Oversized { .. } | FrameError::UnknownKind(_)) => {
				Err(invalid(
					"protocol.invalid_frame",
					"the frame violated the protocol limits",
				))
			}
			Err(FrameError::Closed | FrameError::Io(_)) => {
				Err(ReceiveError::Disconnected)
			}
		}
	}
}

fn answer(
	core: &Core,
	actor: &Actor,
	id: RequestId,
	query: &jet_protocol::QueryRequest,
) -> ServerMessage {
	match core
		.query(actor, translate::query(query))
		.and_then(translate::query_result)
	{
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
	command: &jet_protocol::CommandRequest,
	request_bytes: &[u8],
) -> ServerMessage {
	match core.execute(
		actor,
		CommandEnvelope::new(
			CommandId(command_id),
			translate::command(command),
			request_bytes,
		),
	) {
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
	}
}

fn frame_limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}
