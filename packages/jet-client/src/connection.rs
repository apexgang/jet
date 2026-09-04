//! One authenticated Jet protocol connection.

use std::path::Path;

use jet_protocol::{
	CODEC_JSON_V1, CONNECTION_STREAM, ClientHello, ClientMessage,
	CommandRequest, CommandResponse, ControlError, Frame, FrameError,
	FrameLimits, FrameReader, FrameWriter, MULTIPLEXED_STREAMS_MINOR,
	PROTOCOL_MINOR, PROTOCOL_VERSION, QueryRequest, QueryResponse, RequestId,
	ServerHello, ServerMessage, StreamId, VersionRange, WireError,
	decode_control, encode_control,
};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use uuid::Uuid;

/// Failure while connecting to or talking with `jetd`.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
	/// The socket could not be reached.
	#[error("connection failed: {0}")]
	Io(#[from] std::io::Error),
	/// The byte stream violated the framing rules.
	#[error(transparent)]
	Frame(#[from] FrameError),
	/// A control payload could not be decoded.
	#[error(transparent)]
	Control(#[from] ControlError),
	/// The daemon refused the handshake.
	#[error("handshake rejected: {0:?}")]
	Rejected(WireError),
	/// The daemon accepted the handshake with a protocol this client does
	/// not speak (ADR-0019).
	#[error("incompatible protocol {protocol}.{minor} with codec {codec}")]
	Incompatible {
		/// The selected protocol major.
		protocol: u32,
		/// The selected minor of that major.
		minor: u32,
		/// The selected codec.
		codec: String,
	},
	/// The connected daemon negotiated an older minor than a request needs.
	#[error(
		"feature requires protocol minor {required_minor}, but the connection negotiated {negotiated_minor}"
	)]
	FeatureUnavailable {
		/// First minor that supports the requested feature.
		required_minor: u32,
		/// Minor selected during the handshake.
		negotiated_minor: u32,
	},
	/// The daemon answered a request with a stable error.
	#[error("request failed: {0:?}")]
	Remote(WireError),
	/// The daemon ended the connection with a stable error instead of
	/// answering, for example while draining before shutdown (ADR-0088).
	#[error("connection ended by jetd: {0:?}")]
	Disconnected(WireError),
	/// The daemon sent something this client cannot use here.
	#[error("unexpected message from jetd: {0}")]
	Unexpected(String),
}

/// A connected, handshaken Jet protocol client.
#[derive(Debug)]
pub struct Client {
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
	next_id: RequestId,
	next_stream_id: u32,
	minor: u32,
}

impl Client {
	/// Connects to the local `jetd` socket and completes the handshake as
	/// the installation identified by `client_id`.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Rejected`] when the daemon refuses the
	/// handshake, [`ClientError::Incompatible`] when it selects a protocol
	/// this client does not speak, or the transport or framing failure
	/// otherwise.
	pub async fn connect_local(
		socket: &Path,
		client_id: Uuid,
	) -> Result<Self, ClientError> {
		let mut stream = UnixStream::connect(socket).await?;
		stream.write_all(jet_protocol::PREFACE).await?;
		let (read, write) = stream.into_split();
		let mut client = Self {
			reader: FrameReader::new(read),
			writer: FrameWriter::new(write),
			next_id: 1,
			next_stream_id: 1,
			minor: 0,
		};
		let accepted = FrameLimits::default();
		let hello = ClientHello {
			protocol: VersionRange {
				min: PROTOCOL_VERSION,
				max: PROTOCOL_VERSION,
			},
			minor: PROTOCOL_MINOR,
			codec: CODEC_JSON_V1.into(),
			client_id,
			max_control_frame: limit(accepted.control),
			max_data_frame: limit(accepted.data),
			capabilities: vec![],
		};
		client.send_on(CONNECTION_STREAM, &hello).await?;
		let (stream_id, hello) = client.receive::<ServerHello>().await?;
		if !stream_id.is_connection() {
			return Err(ClientError::Unexpected(
				"handshake reply arrived on an application stream".into(),
			));
		}
		match hello {
			ServerHello::Welcome {
				protocol,
				minor,
				codec,
				max_control_frame,
				max_data_frame,
				capabilities: _,
			} => {
				if protocol != PROTOCOL_VERSION
					|| minor > PROTOCOL_MINOR
					|| codec != CODEC_JSON_V1
				{
					return Err(ClientError::Incompatible {
						protocol,
						minor,
						codec,
					});
				}
				// The peer's limits never raise this side above the protocol
				// maxima (ADR-0089).
				client.writer.set_limits(accepted.negotiate(FrameLimits {
					control: max_control_frame as usize,
					data: max_data_frame as usize,
				}));
				if minor >= MULTIPLEXED_STREAMS_MINOR {
					client.reader.enable_multiplexing();
					client.writer.enable_multiplexing();
				}
				client.minor = minor;
				Ok(client)
			}
			ServerHello::Rejected { error } => {
				Err(ClientError::Rejected(error))
			}
		}
	}

	pub(crate) fn require_minor(
		&self,
		required_minor: u32,
	) -> Result<(), ClientError> {
		if self.minor >= required_minor {
			Ok(())
		} else {
			Err(ClientError::FeatureUnavailable {
				required_minor,
				negotiated_minor: self.minor,
			})
		}
	}

	pub(crate) fn negotiated_minor(&self) -> u32 {
		self.minor
	}

	/// Runs `query` and returns its snapshot.
	pub(crate) async fn query(
		&mut self,
		query: QueryRequest,
	) -> Result<QueryResponse, ClientError> {
		let id = self.next_id();
		let stream_id = self.request_stream();
		self.send_on(stream_id, &ClientMessage::Query { id, query })
			.await?;
		let (reply_stream, reply) = self.receive::<ServerMessage>().await?;
		validate_reply_stream(stream_id, reply_stream, &reply)?;
		match reply {
			ServerMessage::QueryResult {
				id: reply_id,
				result,
			} => expect_reply_to(id, reply_id, result),
			ServerMessage::Error {
				id: reply_id,
				error,
			} => Err(remote_error(id, reply_id, error)),
			other @ ServerMessage::CommandResult { .. } => {
				Err(ClientError::Unexpected(format!("{other:?}")))
			}
		}
	}

	/// Executes `command` under the Actor-scoped identity `command_id`.
	/// Retrying the same identity with the same content returns the original
	/// durable outcome; the caller must therefore keep `command_id` across
	/// retries (ADR-0093).
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon rejects the Command,
	/// or the transport failure otherwise.
	pub async fn execute_command(
		&mut self,
		command_id: Uuid,
		command: CommandRequest,
	) -> Result<CommandResponse, ClientError> {
		let id = self.next_id();
		let stream_id = self.request_stream();
		self.send_on(
			stream_id,
			&ClientMessage::Command {
				id,
				command_id,
				command,
			},
		)
		.await?;
		let (reply_stream, reply) = self.receive::<ServerMessage>().await?;
		validate_reply_stream(stream_id, reply_stream, &reply)?;
		match reply {
			ServerMessage::CommandResult {
				id: reply_id,
				result,
			} => expect_reply_to(id, reply_id, result),
			ServerMessage::Error {
				id: reply_id,
				error,
			} => Err(remote_error(id, reply_id, error)),
			other @ ServerMessage::QueryResult { .. } => {
				Err(ClientError::Unexpected(format!("{other:?}")))
			}
		}
	}

	fn next_id(&mut self) -> RequestId {
		let id = self.next_id;
		self.next_id += 1;
		id
	}

	fn request_stream(&mut self) -> StreamId {
		if self.minor < MULTIPLEXED_STREAMS_MINOR {
			return CONNECTION_STREAM;
		}
		let stream_id = StreamId::new(self.next_stream_id)
			.expect("application stream IDs are never zero");
		self.next_stream_id = self.next_stream_id.wrapping_add(1);
		if self.next_stream_id == 0 {
			self.next_stream_id = 1;
		}
		stream_id
	}

	async fn send_on<T: serde::Serialize>(
		&mut self,
		stream_id: StreamId,
		message: &T,
	) -> Result<(), ClientError> {
		let payload = encode_control(message)?;
		let frame = if stream_id.is_connection() {
			Frame::control(payload)
		} else {
			Frame::stream_control(stream_id, payload)
		};
		self.writer.write(&frame).await?;
		Ok(())
	}

	async fn receive<T: serde::de::DeserializeOwned>(
		&mut self,
	) -> Result<(StreamId, T), ClientError> {
		match self.reader.read().await? {
			Frame::Control { stream_id, payload } => {
				Ok((stream_id, decode_control(&payload)?))
			}
			Frame::Data { .. } => Err(ClientError::Unexpected(
				"data frame before any stream was opened".into(),
			)),
		}
	}
}

fn validate_reply_stream(
	expected: StreamId,
	received: StreamId,
	reply: &ServerMessage,
) -> Result<(), ClientError> {
	if received == expected
		|| (received.is_connection()
			&& matches!(reply, ServerMessage::Error { id: None, .. }))
	{
		Ok(())
	} else {
		Err(ClientError::Unexpected(format!(
			"reply arrived on stream {received:?} while waiting on {expected:?}"
		)))
	}
}

/// Accepts `result` only when it answers request `id`.
fn expect_reply_to<T: std::fmt::Debug>(
	id: RequestId,
	reply_id: RequestId,
	result: T,
) -> Result<T, ClientError> {
	if reply_id == id {
		Ok(result)
	} else {
		Err(ClientError::Unexpected(format!(
			"reply to request {reply_id} while waiting for {id}: {result:?}"
		)))
	}
}

/// Classifies an error frame received while waiting for the reply to `id`.
fn remote_error(
	id: RequestId,
	reply_id: Option<RequestId>,
	error: WireError,
) -> ClientError {
	match reply_id {
		Some(reply_id) if reply_id == id => ClientError::Remote(error),
		Some(reply_id) => ClientError::Unexpected(format!(
			"error for request {reply_id} while waiting for {id}: {error:?}"
		)),
		None => ClientError::Disconnected(error),
	}
}

fn limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
