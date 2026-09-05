//! One authenticated Jet protocol connection.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use jet_protocol::{
	CODEC_JSON_V1, CONNECTION_STREAM, ClientMessage, CommandRequest,
	CommandResponse, ControlError, Frame, FrameError, FrameLimits, FrameReader,
	FrameWriter, MULTIPLEXED_STREAMS_MINOR, PROTOCOL_MINOR, PROTOCOL_VERSION,
	QueryRequest, QueryResponse, RequestId, ServerHello, ServerMessage,
	StreamId, WireError, decode_control, encode_control,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Keeps one client from allocating an unbounded pending-reply registry.
const MAX_IN_FLIGHT_REQUESTS: usize = 256;

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
	/// The connection ended before a pending request received its reply.
	#[error("connection closed before jetd replied")]
	Closed,
}

/// A connected, handshaken Jet protocol client.
#[derive(Debug)]
pub struct Client {
	pub(crate) ssh: Option<tokio::process::Child>,
	outbound: mpsc::Sender<WriteRequest>,
	pending: PendingReplies,
	reader_task: JoinHandle<()>,
	writer_task: JoinHandle<()>,
	legacy_request: Semaphore,
	in_flight: Arc<Semaphore>,
	next_id: AtomicU64,
	next_stream_id: AtomicU32,
	minor: u32,
}

type PendingReplies = Arc<Mutex<HashMap<StreamId, PendingReply>>>;

#[derive(Debug)]
struct PendingReply {
	reply: oneshot::Sender<ServerMessage>,
	_permit: OwnedSemaphorePermit,
}

struct WriteRequest {
	frame: Frame,
	finished: oneshot::Sender<Result<(), FrameError>>,
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
		let (read, write) = UnixStream::connect(socket).await?.into_split();
		let (reader, writer, hello) =
			crate::handshake::local(read, write, client_id).await?;
		Self::from_handshake(reader, writer, hello)
	}

	pub(crate) fn from_handshake<R, W>(
		mut reader: FrameReader<R>,
		mut writer: FrameWriter<W>,
		hello: ServerHello,
	) -> Result<Self, ClientError>
	where
		R: AsyncRead + Unpin + Send + 'static,
		W: AsyncWrite + Unpin + Send + 'static,
	{
		let accepted = FrameLimits::default();
		match hello {
			ServerHello::Challenge { .. } => Err(ClientError::Unexpected(
				"remote challenge on local connection".into(),
			)),
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
				writer.set_limits(accepted.negotiate(FrameLimits {
					control: max_control_frame as usize,
					data: max_data_frame as usize,
				}));
				if minor >= MULTIPLEXED_STREAMS_MINOR {
					reader.enable_multiplexing();
					writer.enable_multiplexing();
				}
				let pending = PendingReplies::default();
				let (outbound, writes) = mpsc::channel(MAX_IN_FLIGHT_REQUESTS);
				let reader_task =
					tokio::spawn(read_replies(reader, Arc::clone(&pending)));
				let writer_task = tokio::spawn(write_frames(writer, writes));
				Ok(Self {
					ssh: None,
					outbound,
					pending,
					reader_task,
					writer_task,
					legacy_request: Semaphore::new(1),
					in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
					next_id: AtomicU64::new(1),
					next_stream_id: AtomicU32::new(1),
					minor,
				})
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
		&self,
		query: QueryRequest,
	) -> Result<QueryResponse, ClientError> {
		let id = self.next_id();
		let stream_id = self.request_stream();
		let reply = self
			.exchange(stream_id, &ClientMessage::Query { id, query })
			.await?;
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
		&self,
		command_id: Uuid,
		command: CommandRequest,
	) -> Result<CommandResponse, ClientError> {
		let id = self.next_id();
		let stream_id = self.request_stream();
		let reply = self
			.exchange(
				stream_id,
				&ClientMessage::Command {
					id,
					command_id,
					command,
				},
			)
			.await?;
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

	fn next_id(&self) -> RequestId {
		loop {
			let id = self.next_id.fetch_add(1, Ordering::Relaxed);
			if id != 0 {
				return id;
			}
		}
	}

	fn request_stream(&self) -> StreamId {
		if self.minor < MULTIPLEXED_STREAMS_MINOR {
			return CONNECTION_STREAM;
		}
		loop {
			let id = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
			if let Some(stream_id) = StreamId::new(id) {
				return stream_id;
			}
		}
	}

	async fn exchange<T: serde::Serialize>(
		&self,
		stream_id: StreamId,
		message: &T,
	) -> Result<ServerMessage, ClientError> {
		// ASVS 15.2.2 and 15.4.4: bound both the pending-reply registry and
		// the writer channel before encoding another untrusted exchange.
		let permit = Arc::clone(&self.in_flight)
			.acquire_owned()
			.await
			.map_err(|_| ClientError::Closed)?;
		let _legacy_request_permit = if stream_id.is_connection() {
			Some(
				self.legacy_request
					.acquire()
					.await
					.map_err(|_| ClientError::Closed)?,
			)
		} else {
			None
		};
		let (reply, receive) = oneshot::channel();
		{
			let mut pending = self
				.pending
				.lock()
				.expect("the pending-reply registry must not be poisoned");
			if pending.contains_key(&stream_id) {
				return Err(ClientError::Unexpected(format!(
					"stream {stream_id:?} was reused while still active"
				)));
			}
			pending.insert(
				stream_id,
				PendingReply {
					reply,
					_permit: permit,
				},
			);
		}
		if let Err(error) = self.send_on(stream_id, message).await {
			self.pending
				.lock()
				.expect("the pending-reply registry must not be poisoned")
				.remove(&stream_id);
			return Err(error);
		}
		receive.await.map_err(|_| ClientError::Closed)
	}

	async fn send_on<T: serde::Serialize>(
		&self,
		stream_id: StreamId,
		message: &T,
	) -> Result<(), ClientError> {
		let frame = Frame::stream_control(stream_id, encode_control(message)?);
		let (finished, written) = oneshot::channel();
		self.outbound
			.send(WriteRequest { frame, finished })
			.await
			.map_err(|_| ClientError::Closed)?;
		written.await.map_err(|_| ClientError::Closed)??;
		Ok(())
	}
}

impl Drop for Client {
	fn drop(&mut self) {
		self.reader_task.abort();
		self.writer_task.abort();
	}
}

async fn read_replies<R: AsyncRead + Unpin>(
	mut reader: FrameReader<R>,
	pending: PendingReplies,
) {
	loop {
		let Frame::Control { stream_id, payload } = (match reader.read().await {
			Ok(frame) => frame,
			Err(_) => break,
		}) else {
			break;
		};
		let Ok(reply) = decode_control::<ServerMessage>(&payload) else {
			break;
		};
		if stream_id.is_connection()
			&& matches!(reply, ServerMessage::Error { id: None, .. })
		{
			let waiters: Vec<_> = pending
				.lock()
				.expect("the pending-reply registry must not be poisoned")
				.drain()
				.map(|(_, pending)| pending.reply)
				.collect();
			for waiter in waiters {
				let _ = waiter.send(reply.clone());
			}
			break;
		}
		let Some(waiter) = pending
			.lock()
			.expect("the pending-reply registry must not be poisoned")
			.remove(&stream_id)
		else {
			break;
		};
		let _ = waiter.reply.send(reply);
	}
	pending
		.lock()
		.expect("the pending-reply registry must not be poisoned")
		.clear();
}

async fn write_frames<W: AsyncWrite + Unpin>(
	mut writer: FrameWriter<W>,
	mut writes: mpsc::Receiver<WriteRequest>,
) {
	while let Some(WriteRequest { frame, finished }) = writes.recv().await {
		let result = writer.write(&frame).await;
		let failed = result.is_err();
		let _ = finished.send(result);
		if failed {
			return;
		}
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

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
