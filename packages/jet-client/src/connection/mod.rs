//! One authenticated Jet protocol connection.

mod error;
pub use error::ClientError;

pub(crate) mod handshake;
pub(crate) mod ssh;

use jet_protocol::{
	CODEC_JSON_V1, CONNECTION_STREAM, ClientMessage, CommandRequest,
	CommandResponse, Frame, FrameError, FrameLimits, FrameReader, FrameWriter,
	MULTIPLEXED_STREAMS_MINOR, PROTOCOL_MINOR, PROTOCOL_VERSION, QueryRequest,
	QueryResponse, RequestId, ServerHello, ServerMessage, StreamId, WireError,
	decode_control, encode_control,
};
use std::{
	collections::HashMap,
	path::Path,
	sync::{
		Arc, Mutex,
		atomic::{AtomicU32, AtomicU64, Ordering},
	},
};
use tokio::{
	io::{AsyncRead, AsyncWrite},
	net::UnixStream,
	sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
	task::JoinHandle,
};
use uuid::Uuid;

#[path = "artifact_client.rs"]
mod artifact_client;

/// Keeps one client from allocating an unbounded pending-reply registry.
const MAX_IN_FLIGHT_REQUESTS: usize = 256;

/// A connected, handshaken Jet protocol client.
#[derive(Debug)]
pub struct Client {
	transfers: artifact_client::Transfers,
	data_limit: usize,
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
	/// Executes a bounded destination operation under this signed connection.
	/// Preserve the operation identity on retry; a lost mutation reply is uncertain.
	/// # Errors
	/// Returns a destination refusal, incompatible minor, or transport failure.
	pub async fn remote_tool(
		&self,
		request: jet_protocol::RemoteToolRequest,
	) -> Result<jet_protocol::RemoteToolResult, ClientError> {
		self.require_minor(jet_protocol::NO_VISA_MINOR)?;
		let id = self.next_id();
		let reply = self
			.exchange(
				self.request_stream(),
				&ClientMessage::RemoteTool { id, request },
			)
			.await?;
		match reply {
			ServerMessage::RemoteToolResult {
				id: reply_id,
				result,
			} => expect_reply_to(id, reply_id, result),
			ServerMessage::Error {
				id: reply_id,
				error,
			} => Err(remote_error(id, reply_id, error)),
			other @ (ServerMessage::QueryResult { .. }
			| ServerMessage::CommandResult { .. }
			| ServerMessage::TerminalAttached { .. }
			| ServerMessage::TerminalResized { .. }) => {
				Err(ClientError::Unexpected(format!("{other:?}")))
			}
		}
	}
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
			crate::connection::handshake::local(read, write, client_id).await?;
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
				let transfers = artifact_client::Transfers::default();
				let data_limit = writer.limits().data;
				let (outbound, writes) = mpsc::channel(MAX_IN_FLIGHT_REQUESTS);
				let reader_task = tokio::spawn(read_replies(
					reader,
					Arc::clone(&pending),
					Arc::clone(&transfers),
				));
				let writer_task = tokio::spawn(write_frames(writer, writes));
				Ok(Self {
					transfers,
					data_limit,
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
			.exchange(
				stream_id,
				&ClientMessage::Query {
					id,
					query,
					timeout_ms: None,
				},
			)
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
			other @ (ServerMessage::CommandResult { .. }
			| ServerMessage::TerminalAttached { .. }
			| ServerMessage::TerminalResized { .. }
			| ServerMessage::RemoteToolResult { .. }) => {
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
			other @ (ServerMessage::QueryResult { .. }
			| ServerMessage::TerminalAttached { .. }
			| ServerMessage::TerminalResized { .. }
			| ServerMessage::RemoteToolResult { .. }) => {
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
		self.send_frame(frame).await
	}

	async fn send_frame(&self, frame: Frame) -> Result<(), ClientError> {
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
	transfers: artifact_client::Transfers,
) {
	loop {
		let frame = match reader.read().await {
			Ok(frame) => frame,
			Err(_) => break,
		};
		match artifact_client::route(&transfers, &frame) {
			Ok(true) => continue,
			Ok(false) => {}
			Err(()) => break,
		}
		let Frame::Control { stream_id, payload } = frame else {
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
	transfers.lock().expect("Artifact replies").clear();
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
mod tests {
	use jet_protocol::{
		ClientHello, ClientMessage, Frame, FrameReader, FrameWriter,
		PageCursor, PlaneStatus, QueryRequest, QueryResponse, ServerHello,
		ServerMessage, VersionRange, decode_control, encode_control,
	};
	use pretty_assertions::assert_eq;
	use tokio::io::AsyncReadExt;
	use tokio::net::UnixListener;
	use tokio::sync::{mpsc, oneshot};
	use tokio::time::{Duration, timeout};
	use uuid::Uuid;

	use super::{Client, ClientError, MAX_IN_FLIGHT_REQUESTS};

	#[tokio::test]
	async fn a_new_client_keeps_the_minor_zero_contract_with_an_old_daemon() {
		let dir = tempfile::tempdir().unwrap();
		let socket = dir.path().join("old-jetd.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let server = tokio::spawn(async move {
			let (mut stream, _) = listener.accept().await.unwrap();
			let mut preface = vec![0; jet_protocol::PREFACE.len()];
			stream.read_exact(&mut preface).await.unwrap();
			assert_eq!(preface, jet_protocol::PREFACE);
			let (read, write) = stream.into_split();
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			let Frame::Control { payload: hello, .. } =
				reader.read().await.unwrap()
			else {
				panic!("expected a control frame");
			};
			let hello: ClientHello = decode_control(&hello).unwrap();
			assert_eq!(hello.protocol, VersionRange { min: 1, max: 1 });
			writer
				.write(&Frame::control(
					encode_control(&ServerHello::Welcome {
						protocol: 1,
						minor: 0,
						codec: "json-v1".into(),
						max_control_frame: 1_048_576,
						max_data_frame: 262_144,
						capabilities: vec![],
					})
					.unwrap(),
				))
				.await
				.unwrap();
			let Frame::Control { .. } = reader.read().await.unwrap() else {
				panic!("expected the status Query");
			};
			writer
				.write(&Frame::control(
					br#"{"kind":"query_result","id":1,"result":{"type":"status","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":1,"started_at_unix_ms":0,"core_version":"0.1.0"}}"#
						.to_vec(),
				))
				.await
				.unwrap();
		});

		let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
		let status = client.status().await.unwrap();
		let unavailable = client
			.next_conversations(PageCursor(Uuid::nil()))
			.await
			.unwrap_err();

		assert_eq!(status.cursor, None);
		assert!(matches!(
			unavailable,
			ClientError::FeatureUnavailable {
				required_minor: 1,
				negotiated_minor: 0
			}
		));
		server.await.unwrap();
	}

	#[tokio::test]
	async fn a_current_client_switches_to_numbered_streams_after_the_handshake()
	{
		let dir = tempfile::tempdir().unwrap();
		let socket = dir.path().join("jetd.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let server = tokio::spawn(async move {
			let (mut stream, _) = listener.accept().await.unwrap();
			let mut preface = vec![0; jet_protocol::PREFACE.len()];
			stream.read_exact(&mut preface).await.unwrap();
			let (read, write) = stream.into_split();
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			let Frame::Control { .. } = reader.read().await.unwrap() else {
				panic!("expected a control-frame hello");
			};
			writer
				.write(&Frame::control(
					encode_control(&ServerHello::Welcome {
						protocol: jet_protocol::PROTOCOL_VERSION,
						minor: jet_protocol::PROTOCOL_MINOR,
						codec: jet_protocol::CODEC_JSON_V1.into(),
						max_control_frame: 1_048_576,
						max_data_frame: 262_144,
						capabilities: vec![],
					})
					.unwrap(),
				))
				.await
				.unwrap();
			reader.enable_multiplexing();
			writer.enable_multiplexing();

			let Frame::Control { stream_id, payload } =
				reader.read().await.unwrap()
			else {
				panic!("expected a control-frame Query");
			};
			let request: jet_protocol::ClientMessage =
				decode_control(&payload).unwrap();
			assert!(matches!(
				request,
				jet_protocol::ClientMessage::Query {
					id: 1,
					query: QueryRequest::Status,
					..
				}
			));
			assert!(!stream_id.is_connection());
			writer
				.write(&Frame::stream_control(
					stream_id,
					encode_control(&ServerMessage::QueryResult {
						id: 1,
						result: QueryResponse::Status(PlaneStatus {
							cursor: Some(0),
							plane_id: Uuid::nil(),
							daemon_starts: 1,
							started_at_unix_ms: 0,
							core_version: "0.2.0".into(),
							security: None,
							recovery: None,
						}),
					})
					.unwrap(),
				))
				.await
				.unwrap();
		});

		let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
		let status = client.status().await.unwrap();
		assert_eq!(
			status,
			PlaneStatus {
				cursor: Some(0),
				plane_id: Uuid::nil(),
				daemon_starts: 1,
				started_at_unix_ms: 0,
				core_version: "0.2.0".into(),
				security: None,
				recovery: None,
			}
		);
		server.await.unwrap();
	}

	#[tokio::test]
	async fn concurrent_requests_are_demultiplexed_by_numbered_stream() {
		let dir = tempfile::tempdir().unwrap();
		let socket = dir.path().join("multiplexed-jetd.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let server = tokio::spawn(async move {
			let (mut stream, _) = listener.accept().await.unwrap();
			let mut preface = vec![0; jet_protocol::PREFACE.len()];
			stream.read_exact(&mut preface).await.unwrap();
			let (read, write) = stream.into_split();
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			assert!(matches!(
				reader.read().await.unwrap(),
				Frame::Control { .. }
			));
			writer
				.write(&Frame::control(
					encode_control(&ServerHello::Welcome {
						protocol: jet_protocol::PROTOCOL_VERSION,
						minor: jet_protocol::PROTOCOL_MINOR,
						codec: jet_protocol::CODEC_JSON_V1.into(),
						max_control_frame: 1_048_576,
						max_data_frame: 262_144,
						capabilities: vec![],
					})
					.unwrap(),
				))
				.await
				.unwrap();
			reader.enable_multiplexing();
			writer.enable_multiplexing();

			let mut requests = Vec::new();
			for _ in 0..2 {
				let Frame::Control { stream_id, payload } =
					reader.read().await.unwrap()
				else {
					panic!("expected a control-frame Query");
				};
				let ClientMessage::Query {
					id,
					query: QueryRequest::Status,
					..
				} = decode_control(&payload).unwrap()
				else {
					panic!("expected a status Query");
				};
				requests.push((stream_id, id));
			}
			for (stream_id, id) in requests.into_iter().rev() {
				writer
					.write(&Frame::stream_control(
						stream_id,
						encode_control(&ServerMessage::QueryResult {
							id,
							result: QueryResponse::Status(PlaneStatus {
								cursor: Some(id),
								plane_id: Uuid::nil(),
								daemon_starts: id,
								started_at_unix_ms: 0,
								core_version: format!("reply-{id}"),
								security: None,
								recovery: None,
							}),
						})
						.unwrap(),
					))
					.await
					.unwrap();
			}
		});

		let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
		let (first, second) = tokio::join!(client.status(), client.status());
		assert_eq!(
			(first.unwrap(), second.unwrap()),
			(
				PlaneStatus {
					cursor: Some(1),
					plane_id: Uuid::nil(),
					daemon_starts: 1,
					started_at_unix_ms: 0,
					core_version: "reply-1".into(),
					security: None,
					recovery: None,
				},
				PlaneStatus {
					cursor: Some(2),
					plane_id: Uuid::nil(),
					daemon_starts: 2,
					started_at_unix_ms: 0,
					core_version: "reply-2".into(),
					security: None,
					recovery: None,
				},
			)
		);
		server.await.unwrap();
	}

	#[tokio::test]
	async fn canceled_requests_remain_bounded_until_their_replies_arrive() {
		let dir = tempfile::tempdir().unwrap();
		let socket = dir.path().join("canceled-jetd.sock");
		let listener = UnixListener::bind(&socket).unwrap();
		let (seen, mut received) = mpsc::channel(1);
		let (release, released) = oneshot::channel();
		let server = tokio::spawn(async move {
			let (mut stream, _) = listener.accept().await.unwrap();
			let mut preface = vec![0; jet_protocol::PREFACE.len()];
			stream.read_exact(&mut preface).await.unwrap();
			let (read, write) = stream.into_split();
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			assert!(matches!(
				reader.read().await.unwrap(),
				Frame::Control { .. }
			));
			writer
				.write(&Frame::control(
					encode_control(&ServerHello::Welcome {
						protocol: jet_protocol::PROTOCOL_VERSION,
						minor: jet_protocol::PROTOCOL_MINOR,
						codec: jet_protocol::CODEC_JSON_V1.into(),
						max_control_frame: 1_048_576,
						max_data_frame: 262_144,
						capabilities: vec![],
					})
					.unwrap(),
				))
				.await
				.unwrap();
			reader.enable_multiplexing();
			writer.enable_multiplexing();

			let mut requests = Vec::new();
			for _ in 0..MAX_IN_FLIGHT_REQUESTS {
				let Frame::Control { stream_id, payload } =
					reader.read().await.unwrap()
				else {
					panic!("expected a control-frame Query");
				};
				let ClientMessage::Query {
					id,
					query: QueryRequest::Status,
					..
				} = decode_control(&payload).unwrap()
				else {
					panic!("expected a status Query");
				};
				requests.push((stream_id, id));
				seen.send(()).await.unwrap();
			}
			released.await.unwrap();
			for (stream_id, id) in requests {
				writer
					.write(&Frame::stream_control(
						stream_id,
						encode_control(&status_reply(id)).unwrap(),
					))
					.await
					.unwrap();
			}
			let Frame::Control { stream_id, payload } =
				reader.read().await.unwrap()
			else {
				panic!("expected a final control-frame Query");
			};
			let ClientMessage::Query {
				id,
				query: QueryRequest::Status,
				..
			} = decode_control(&payload).unwrap()
			else {
				panic!("expected a final status Query");
			};
			writer
				.write(&Frame::stream_control(
					stream_id,
					encode_control(&status_reply(id)).unwrap(),
				))
				.await
				.unwrap();
		});
		let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();

		for _ in 0..MAX_IN_FLIGHT_REQUESTS {
			let request = client.status();
			tokio::pin!(request);
			timeout(Duration::from_secs(1), async {
				tokio::select! {
					result = &mut request => panic!("request unexpectedly ended: {result:?}"),
					seen = received.recv() => assert_eq!(seen, Some(())),
				}
			})
			.await
			.expect("the bounded request must reach the peer");
		}
		{
			let blocked = client.status();
			tokio::pin!(blocked);
			assert!(
				timeout(Duration::from_millis(100), async {
					tokio::select! {
						result = &mut blocked => panic!("request unexpectedly ended: {result:?}"),
						seen = received.recv() => panic!("request exceeded the bound: {seen:?}"),
					}
				})
				.await
				.is_err()
			);
		}
		release.send(()).unwrap();
		assert_eq!(
			timeout(Duration::from_secs(2), client.status())
				.await
				.unwrap()
				.unwrap(),
			status()
		);
		server.await.unwrap();
	}

	fn status_reply(id: u64) -> ServerMessage {
		ServerMessage::QueryResult {
			id,
			result: QueryResponse::Status(status()),
		}
	}

	fn status() -> PlaneStatus {
		PlaneStatus {
			cursor: Some(0),
			plane_id: Uuid::nil(),
			daemon_starts: 1,
			started_at_unix_ms: 0,
			core_version: "0.2.0".into(),
			security: None,
			recovery: None,
		}
	}
}
