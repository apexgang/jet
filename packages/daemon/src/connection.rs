//! Serves one local Jet protocol connection: preface, handshake, requests.

use std::sync::Arc;
use std::time::Duration;

use jet_core::{Actor, ClientId, Core};
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ClientMessage, ErrorCategory, Frame,
	FrameError, FrameReader, FrameWriter, MAX_CONTROL_FRAME, MAX_DATA_FRAME,
	PREFACE, PROTOCOL_VERSION, RequestId, ServerHello, ServerMessage,
	WireError, decode_control, encode_control,
};
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
		} else {
			None
		};
		if let Some(error) = rejection {
			let _ = self.send(&ServerHello::Rejected { error }).await;
			return None;
		}
		let welcome = ServerHello::Welcome {
			protocol: PROTOCOL_VERSION,
			codec: CODEC_JSON_V1.into(),
			max_control_frame: frame_limit(MAX_CONTROL_FRAME),
			max_data_frame: frame_limit(MAX_DATA_FRAME),
		};
		self.send(&welcome).await.ok()?;
		Some(Actor::InteractiveClient {
			client_id: ClientId(hello.client_id),
		})
	}

	async fn serve_requests(&mut self, core: &Core, actor: &Actor) {
		loop {
			let message: ClientMessage = match self.receive().await {
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
				ClientMessage::Query { id, query } => {
					answer(core, actor, id, &query)
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
	match core.query(actor, translate::query(query)) {
		Ok(result) => ServerMessage::QueryResult {
			id,
			result: translate::query_result(result),
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
	}
}

fn frame_limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}
