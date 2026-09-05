//! Serves one local Jet protocol connection: preface, handshake, requests,
//! drain.

use std::sync::Arc;
use std::time::Duration;

use jet_core::{Actor, ClientId, CommandEnvelope, CommandId, Core};
use jet_protocol::{
	CODEC_JSON_V1, CONNECTION_STREAM, ClientHello, CommandRequest,
	ErrorCategory, Frame, FrameError, FrameLimits, FrameReader, FrameWriter,
	MULTIPLEXED_STREAMS_MINOR, PREFACE, PROTOCOL_MINOR, PROTOCOL_VERSION,
	QueryRequest, RequestId, ServerHello, ServerMessage, StreamId, WireError,
	decode_control, encode_control,
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
	draining: watch::Receiver<bool>,
) {
	let Ok(Some((connection, actor, minor))) =
		timeout(HANDSHAKE_TIMEOUT, open(stream)).await
	else {
		return;
	};
	connection
		.serve_requests(core, actor, minor, draining)
		.await;
}

async fn open(mut stream: UnixStream) -> Option<(Connection, Actor, u32)> {
	let mut preface = vec![0u8; PREFACE.len()];
	if stream.read_exact(&mut preface).await.is_err() || preface != PREFACE {
		return None;
	}
	let (read, write) = stream.into_split();
	let mut connection = Connection {
		reader: FrameReader::new(read),
		writer: FrameWriter::new(write),
	};
	let (actor, minor) = connection.handshake().await?;
	Some((connection, actor, minor))
}

impl Connection {
	async fn handshake(&mut self) -> Option<(Actor, u32)> {
		let hello: ClientHello = match self.receive().await {
			Ok((stream_id, hello)) if stream_id.is_connection() => hello,
			Ok(_) => return None,
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
		if minor >= MULTIPLEXED_STREAMS_MINOR {
			self.reader.enable_multiplexing();
			self.writer.enable_multiplexing();
		}
		Some((
			Actor::InteractiveClient {
				client_id: ClientId(hello.client_id),
			},
			minor,
		))
	}

	/// Answers requests until the peer leaves or the daemon drains. A request
	/// already received is answered before the drain is honored, so an
	/// accepted Command is never left without its reply (ADR-0088).
	async fn serve_requests(
		self,
		core: Arc<Core>,
		actor: Actor,
		minor: u32,
		draining: watch::Receiver<bool>,
	) {
		let Self { reader, writer } = self;
		crate::connection_session::serve(
			reader, writer, core, actor, minor, draining,
		)
		.await;
	}

	async fn send<T: serde::Serialize>(
		&mut self,
		message: &T,
	) -> Result<(), SendError> {
		self.send_on(CONNECTION_STREAM, message).await
	}

	async fn send_on<T: serde::Serialize>(
		&mut self,
		stream_id: StreamId,
		message: &T,
	) -> Result<(), SendError> {
		let payload = encode_control(message)?;
		let frame = Frame::stream_control(stream_id, payload);
		self.writer.write(&frame).await?;
		Ok(())
	}

	/// Receives one decoded control message.
	async fn receive<T: serde::de::DeserializeOwned>(
		&mut self,
	) -> Result<(StreamId, T), ReceiveError> {
		let (stream_id, payload) = self.receive_control().await?;
		Ok((stream_id, decode(&payload)?))
	}

	/// Receives the payload of one control frame. Native decoder detail
	/// stays local so only stable, safe messages reach the peer (ADR-0068).
	async fn receive_control(
		&mut self,
	) -> Result<(StreamId, Vec<u8>), ReceiveError> {
		match self.reader.read().await {
			Ok(Frame::Control { stream_id, payload }) => {
				Ok((stream_id, payload))
			}
			Ok(Frame::Data { .. }) => Err(ReceiveError::Protocol(wire_error(
				ErrorCategory::InvalidInput,
				"protocol.unexpected_data_frame",
				"no data stream is open on this connection".into(),
			))),
			Err(
				FrameError::Oversized { .. }
				| FrameError::UnknownKind(_)
				| FrameError::InvalidStream { .. }
				| FrameError::MultiplexingDisabled(_),
			) => Err(ReceiveError::Protocol(wire_error(
				ErrorCategory::InvalidInput,
				"protocol.invalid_frame",
				"the frame violated the protocol limits".into(),
			))),
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

pub(super) async fn answer(
	core: &Core,
	actor: &Actor,
	minor: u32,
	id: RequestId,
	query: &QueryRequest,
) -> ServerMessage {
	if let Some(requirement) = query_minor(query)
		&& minor < requirement.minor
	{
		return unsupported_minor(id, requirement);
	}
	let result = core
		.query(actor, translate::query(query, minor))
		.await
		.and_then(|result| translate::query_result(result, minor));
	match result {
		Ok(result) => ServerMessage::QueryResult { id, result },
		Err(error) => ServerMessage::Error {
			id: Some(id),
			error: translate::error(error, minor),
		},
	}
}

pub(super) async fn execute(
	core: &Core,
	actor: &Actor,
	minor: u32,
	id: RequestId,
	command_id: uuid::Uuid,
	command: &CommandRequest,
	request_bytes: &[u8],
) -> ServerMessage {
	if let Some(requirement) = command_minor(command)
		&& minor < requirement.minor
	{
		return unsupported_minor(id, requirement);
	}
	let envelope = CommandEnvelope::new(
		CommandId(command_id),
		translate::command(command),
		request_bytes,
	);
	let outcome = match envelope {
		Ok(envelope) => core.execute(actor, envelope).await,
		Err(error) => Err(error),
	};
	match outcome {
		Ok(outcome) => ServerMessage::CommandResult {
			id,
			result: translate::command_outcome(outcome),
		},
		Err(error) => ServerMessage::Error {
			id: Some(id),
			error: translate::error(error, minor),
		},
	}
}

/// The protocol minor one request needs, named as the refusal spells it.
struct MinorRequirement {
	minor: u32,
	feature: &'static str,
}

fn query_minor(query: &QueryRequest) -> Option<MinorRequirement> {
	match query {
		QueryRequest::NextConversations { .. } => Some(MinorRequirement {
			minor: jet_protocol::FENCED_READS_MINOR,
			feature: "Conversation pagination",
		}),
		QueryRequest::Settings { .. } => Some(MinorRequirement {
			minor: jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR,
			feature: "Setting Queries",
		}),
		QueryRequest::Capabilities { .. } => Some(MinorRequirement {
			minor: jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR,
			feature: "the Capability Query",
		}),
		QueryRequest::Status
		| QueryRequest::Conversations
		| QueryRequest::Conversation { .. }
		| QueryRequest::Events { .. } => None,
	}
}

fn command_minor(command: &CommandRequest) -> Option<MinorRequirement> {
	match command {
		CommandRequest::SetSetting { .. }
		| CommandRequest::ClearSetting { .. } => Some(MinorRequirement {
			minor: jet_protocol::SETTINGS_AND_CAPABILITIES_MINOR,
			feature: "Setting Commands",
		}),
		CommandRequest::CreateConversation { .. }
		| CommandRequest::CreateRun { .. }
		| CommandRequest::TransitionRun { .. } => None,
	}
}

fn unsupported_minor(
	id: RequestId,
	requirement: MinorRequirement,
) -> ServerMessage {
	let MinorRequirement { minor, feature } = requirement;
	ServerMessage::Error {
		id: Some(id),
		error: wire_error(
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			format!("{feature} needs protocol minor {minor}"),
		),
	}
}

pub(super) fn draining_error() -> WireError {
	WireError {
		category: ErrorCategory::Unavailable,
		code: "daemon.draining".into(),
		retryable: true,
		message: "jetd is shutting down; reconnect later and retry with the same Command identity".into(),
		revision_conflict: None,
		restart: None,
		recovery_actions: vec![],
	}
}

pub(super) fn malformed() -> WireError {
	wire_error(
		ErrorCategory::InvalidInput,
		"protocol.malformed",
		"the control frame is not a valid message".into(),
	)
}

pub(super) fn wire_error(
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
		restart: None,
		recovery_actions: vec![],
	}
}

fn frame_limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}
