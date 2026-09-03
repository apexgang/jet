//! Serves one local Jet protocol connection: preface, handshake, requests.

use std::sync::Arc;

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

use crate::translate;

struct Connection {
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
}

pub(crate) async fn serve(core: Arc<Core>, mut stream: UnixStream) {
	let mut preface = vec![0u8; PREFACE.len()];
	if stream.read_exact(&mut preface).await.is_err() || preface != PREFACE {
		return;
	}
	let (read, write) = stream.into_split();
	let mut connection = Connection {
		reader: FrameReader::new(read),
		writer: FrameWriter::new(write),
	};
	let Some(actor) = connection.handshake().await else {
		return;
	};
	connection.serve_requests(&core, &actor).await;
}

impl Connection {
	async fn handshake(&mut self) -> Option<Actor> {
		let hello: ClientHello = match self.receive().await {
			Ok(hello) => hello,
			Err(error) => {
				let _ = self.send(&ServerHello::Rejected { error }).await;
				return None;
			}
		};
		let rejection = if hello.codec != CODEC_JSON_V1 {
			Some(incompatible(
				"protocol.unsupported_codec",
				format!("only the {CODEC_JSON_V1} codec is supported"),
			))
		} else if !hello.protocol.contains(PROTOCOL_VERSION) {
			Some(incompatible(
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
				Err(error) if error.code == "connection.closed" => return,
				Err(error) => {
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

	async fn receive<T: serde::de::DeserializeOwned>(
		&mut self,
	) -> Result<T, WireError> {
		match self.reader.read().await {
			Ok(Frame::Control(payload)) => {
				decode_control(&payload).map_err(|error| WireError {
					category: ErrorCategory::InvalidInput,
					code: "protocol.malformed".into(),
					retryable: false,
					message: format!("malformed control frame: {error}"),
				})
			}
			Ok(Frame::Data(_)) => Err(WireError {
				category: ErrorCategory::InvalidInput,
				code: "protocol.unexpected_data_frame".into(),
				retryable: false,
				message: "no data stream is open on this connection".into(),
			}),
			Err(FrameError::Closed) => Err(WireError {
				category: ErrorCategory::Unavailable,
				code: "connection.closed".into(),
				retryable: false,
				message: "the client closed the connection".into(),
			}),
			Err(FrameError::Oversized { .. } | FrameError::UnknownKind(_)) => {
				Err(WireError {
					category: ErrorCategory::InvalidInput,
					code: "protocol.invalid_frame".into(),
					retryable: false,
					message: "the frame violated the protocol limits".into(),
				})
			}
			Err(FrameError::Io(_)) => Err(WireError {
				category: ErrorCategory::Unavailable,
				code: "connection.failed".into(),
				retryable: true,
				message: "the connection failed".into(),
			}),
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

fn incompatible(code: &str, message: String) -> WireError {
	WireError {
		category: ErrorCategory::Incompatible,
		code: code.into(),
		retryable: false,
		message,
	}
}

fn frame_limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}
