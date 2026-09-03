//! One authenticated Jet protocol connection.

use std::path::Path;

use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ClientMessage, ControlError, Frame, FrameError,
	FrameLimits, FrameReader, FrameWriter, PROTOCOL_VERSION, PlaneStatus,
	QueryRequest, QueryResponse, RequestId, ServerHello, ServerMessage,
	VersionRange, WireError, decode_control, encode_control,
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
	/// The daemon answered a request with an error.
	#[error("request failed: {0:?}")]
	Remote(WireError),
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
}

impl Client {
	/// Connects to the local `jetd` socket and completes the handshake as
	/// the installation identified by `client_id`.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Rejected`] when the daemon refuses the
	/// handshake, or the transport or framing failure otherwise.
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
		};
		let accepted = FrameLimits::default();
		let hello = ClientHello {
			protocol: VersionRange {
				min: PROTOCOL_VERSION,
				max: PROTOCOL_VERSION,
			},
			codec: CODEC_JSON_V1.into(),
			client_id,
			max_control_frame: limit(accepted.control),
			max_data_frame: limit(accepted.data),
			capabilities: vec![],
		};
		client.send(&hello).await?;
		match client.receive::<ServerHello>().await? {
			ServerHello::Welcome {
				protocol,
				codec,
				max_control_frame,
				max_data_frame,
				..
			} if protocol == PROTOCOL_VERSION && codec == CODEC_JSON_V1 => {
				client.writer.set_limits(FrameLimits {
					control: max_control_frame as usize,
					data: max_data_frame as usize,
				});
				Ok(client)
			}
			ServerHello::Welcome {
				protocol, codec, ..
			} => Err(ClientError::Unexpected(format!(
				"negotiated protocol {protocol} with codec {codec}"
			))),
			ServerHello::Rejected { error } => {
				Err(ClientError::Rejected(error))
			}
		}
	}

	/// Runs the status Query and returns the Plane status snapshot.
	///
	/// # Errors
	///
	/// Returns [`ClientError::Remote`] when the daemon reports a stable
	/// error, or the transport failure otherwise.
	pub async fn status(&mut self) -> Result<PlaneStatus, ClientError> {
		let id = self.next_id;
		self.next_id += 1;
		self.send(&ClientMessage::Query {
			id,
			query: QueryRequest::Status,
		})
		.await?;
		match self.receive::<ServerMessage>().await? {
			ServerMessage::QueryResult {
				id: reply_id,
				result: QueryResponse::Status(status),
			} if reply_id == id => Ok(status),
			ServerMessage::Error {
				id: reply_id,
				error,
			} if reply_id == Some(id) => Err(ClientError::Remote(error)),
			other => Err(ClientError::Unexpected(format!("{other:?}"))),
		}
	}

	async fn send<T: serde::Serialize>(
		&mut self,
		message: &T,
	) -> Result<(), ClientError> {
		let payload = encode_control(message)?;
		self.writer.write(&Frame::Control(payload)).await?;
		Ok(())
	}

	async fn receive<T: serde::de::DeserializeOwned>(
		&mut self,
	) -> Result<T, ClientError> {
		match self.reader.read().await? {
			Frame::Control(payload) => Ok(decode_control(&payload)?),
			Frame::Data(_) => Err(ClientError::Unexpected(
				"data frame before any stream was opened".into(),
			)),
		}
	}
}

fn limit(limit: usize) -> u32 {
	u32::try_from(limit).unwrap_or(u32::MAX)
}
