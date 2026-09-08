//! Concurrent read, request, and prioritized-write loops for one connection.

use std::sync::Arc;

use jet_core::{Actor, Core};
use jet_protocol::{
	ClientMessage, ErrorCategory, Frame, FrameError, FrameReader, FrameWriter,
	MULTIPLEXED_STREAMS_MINOR, OutboundQueue, ServerMessage, StreamControl,
	StreamId, WireError, decode_control, encode_control, raw_command,
};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, watch};

use crate::connection::{
	answer, draining_error, execute, malformed, wire_error,
};

/// Bounds decoded requests waiting behind durable core work.
const MAX_PENDING_REQUESTS: usize = 16;
/// Bounds replies waiting to enter the byte-bounded priority scheduler.
const MAX_PENDING_REPLIES: usize = 16;

enum Request {
	Control {
		stream_id: StreamId,
		payload: Vec<u8>,
		message: Box<ClientMessage>,
	},
	Data {
		stream_id: StreamId,
		payload: Vec<u8>,
	},
	Credit {
		stream_id: StreamId,
		bytes: u64,
	},
}

enum Stop {
	Disconnected,
	Draining,
	Protocol(Box<WireError>),
}

pub(super) async fn serve(
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
	core: Arc<Core>,
	actor: Actor,
	minor: u32,
	draining: watch::Receiver<bool>,
	capacity: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
	let data_limit = writer.limits().data.min(65536) as u32;
	let (request_tx, request_rx) = mpsc::channel(MAX_PENDING_REQUESTS);
	let (reply_tx, reply_rx) = mpsc::channel(MAX_PENDING_REPLIES);
	let drive_inbound = async {
		let (stop, ()) = tokio::join!(
			read_requests(reader, minor, draining, request_tx),
			process_requests(
				core,
				actor,
				minor,
				request_rx,
				reply_tx.clone(),
				capacity,
				data_limit
			),
		);
		let final_error = match stop {
			Stop::Disconnected => None,
			Stop::Draining => Some(draining_error()),
			Stop::Protocol(error) => Some(*error),
		};
		if let Some(error) = final_error
			&& let Ok(payload) =
				encode_control(&ServerMessage::Error { id: None, error })
		{
			let _ = reply_tx.send(Frame::control(payload)).await;
		}
	};
	let writer = write_replies(writer, reply_rx);
	tokio::pin!(drive_inbound, writer);
	tokio::select! {
		() = &mut drive_inbound => writer.await,
		() = &mut writer => {},
	}
}

async fn read_requests(
	mut reader: FrameReader<OwnedReadHalf>,
	minor: u32,
	mut draining: watch::Receiver<bool>,
	requests: mpsc::Sender<Request>,
) -> Stop {
	loop {
		let frame = tokio::select! {
			biased;
			_ = draining.changed() => return Stop::Draining,
			frame = reader.read() => frame,
		};
		let (stream_id, payload) = match frame {
			Ok(Frame::Control { stream_id, payload }) => (stream_id, payload),
			Ok(Frame::Data { stream_id, payload }) => {
				if requests
					.send(Request::Data { stream_id, payload })
					.await
					.is_err()
				{
					return Stop::Disconnected;
				}
				continue;
			}
			Err(FrameError::Closed | FrameError::Io(_)) => {
				return Stop::Disconnected;
			}
			Err(
				FrameError::Oversized { .. }
				| FrameError::UnknownKind(_)
				| FrameError::InvalidStream { .. }
				| FrameError::MultiplexingDisabled(_),
			) => {
				return Stop::Protocol(Box::new(wire_error(
					ErrorCategory::InvalidInput,
					"protocol.invalid_frame",
					"the frame violated the protocol limits".into(),
				)));
			}
		};
		if minor >= MULTIPLEXED_STREAMS_MINOR && stream_id.is_connection() {
			return Stop::Protocol(Box::new(wire_error(
				ErrorCategory::InvalidInput,
				"protocol.invalid_stream",
				"requests must use a numbered application stream".into(),
			)));
		}
		let message =
			match decode_control(&payload) {
				Ok(message) => message,
				Err(_) => {
					return Stop::Protocol(Box::new(match decode_control(&payload) {
					Ok(StreamControl::Credit { bytes }) => {
						if requests
							.send(Request::Credit { stream_id, bytes })
							.await
							.is_err()
						{
							return Stop::Disconnected;
						}
						continue;
					}
					Ok(
						StreamControl::TerminalGap { .. }
						| StreamControl::TerminalFinished { .. }
						| StreamControl::ArtifactFinished { .. },
					) => wire_error(
						ErrorCategory::InvalidInput,
						"protocol.unexpected_stream_control",
						"the client sent server-originated stream control"
							.into(),
					),
					Err(_) => malformed(),
				}));
				}
			};
		if requests
			.send(Request::Control {
				stream_id,
				payload,
				message: Box::new(message),
			})
			.await
			.is_err()
		{
			return Stop::Disconnected;
		}
	}
}

async fn process_requests(
	core: Arc<Core>,
	actor: Actor,
	minor: u32,
	mut requests: mpsc::Receiver<Request>,
	replies: mpsc::Sender<Frame>,
	capacity: Arc<tokio::sync::OwnedSemaphorePermit>,
	data_limit: u32,
) {
	let mut terminals = crate::terminal_stream::Terminals::new(data_limit);
	while let Some(request) = requests.recv().await {
		let (stream_id, payload, message) = match request {
			Request::Control {
				stream_id,
				payload,
				message,
			} => (stream_id, payload, *message),
			Request::Data { stream_id, payload } => {
				if let Err(error) = terminals
					.input(stream_id, payload, &core, &actor, minor)
					.await
				{
					let reply = ServerMessage::Error { id: None, error };
					if let Ok(payload) = encode_control(&reply) {
						let _ = replies
							.send(Frame::stream_control(stream_id, payload))
							.await;
					}
					return;
				}
				continue;
			}
			Request::Credit { stream_id, bytes } => {
				if let Err(error) = terminals.credit(stream_id, bytes) {
					if let Ok(payload) = encode_control(&ServerMessage::Error {
						id: None,
						error,
					}) {
						let _ = replies
							.send(Frame::stream_control(stream_id, payload))
							.await;
					}
					return;
				}
				continue;
			}
		};
		if let Some(reply) = terminals
			.control(stream_id, &message, &core, &actor, minor, &replies)
			.await
		{
			let Ok(payload) = encode_control(&reply) else {
				return;
			};
			if replies
				.send(Frame::stream_control(stream_id, payload))
				.await
				.is_err()
			{
				return;
			}
			terminals.start(stream_id);
			continue;
		}
		if terminals.contains(stream_id) {
			return;
		}
		// The store runs SQLite on its own worker thread, so the core is
		// awaited here rather than moved onto a blocking thread.
		let request_core = Arc::clone(&core);
		let request_actor = actor.clone();
		let request_capacity = Arc::clone(&capacity);
		// An admitted transaction must finish publishing revocation even if
		// its caller disconnects while SQLite commits (ADR-0071, ADR-0093).
		let Ok(reply) = tokio::spawn(async move {
			let _capacity = request_capacity;
			reply_to(&request_core, &request_actor, minor, payload, message)
				.await
		})
		.await
		else {
			return;
		};
		let Ok(payload) = encode_control(&reply) else {
			return;
		};
		let acknowledged = matches!(reply, ServerMessage::CommandResult { .. });

		// A committed Command dispatches its Effect even if its caller has
		// disconnected before receiving the receipt (ADR-0064).
		if acknowledged {
			let effect_core = Arc::clone(&core);
			tokio::spawn(async move {
				if let Err(error) = effect_core.perform_terminals().await {
					eprintln!("jetd: cannot settle terminals: {error}");
				}
				if let Err(error) = effect_core.perform_runs().await {
					eprintln!("jetd: cannot record Run start outcome: {error}");
				}
				if let Err(error) = effect_core.perform_run_controls().await {
					eprintln!(
						"jetd: cannot carry out an execution control request: {error}"
					);
				}
				if let Err(error) = effect_core.perform_promotions().await {
					eprintln!(
						"jetd: cannot record a Workspace promotion outcome: {error}"
					);
				}
			});
		}
		if replies
			.send(Frame::stream_control(stream_id, payload))
			.await
			.is_err()
		{
			return;
		}
	}
}

async fn reply_to(
	core: &Core,
	actor: &Actor,
	minor: u32,
	payload: Vec<u8>,
	message: ClientMessage,
) -> ServerMessage {
	match message {
		ClientMessage::AttachTerminal { id, .. }
		| ClientMessage::ResizeTerminal { id, .. } => ServerMessage::Error {
			id: Some(id),
			error: malformed(),
		},
		ClientMessage::Query {
			id,
			query,
			timeout_ms,
		} => bounded_query(core, actor, minor, id, query, timeout_ms).await,
		ClientMessage::Command {
			id,
			command_id,
			command,
		} => match raw_command(&payload) {
			Ok(raw) => {
				execute(
					core,
					actor,
					minor,
					id,
					command_id,
					&command,
					raw.get().as_bytes(),
				)
				.await
			}
			Err(_) => ServerMessage::Error {
				id: Some(id),
				error: malformed(),
			},
		},
	}
}

/// Answers a Query under the client's own relative bound.
///
/// A Query is a non-durable read, so giving up on one changes nothing on
/// the Plane. Commands take no bound at all: once a Command is durably
/// accepted, no transport-level cancellation may reverse it, and changing
/// live work needs an explicit Interrupt turn or Stop Run (ADR-0095).
async fn bounded_query(
	core: &Core,
	actor: &Actor,
	minor: u32,
	id: jet_protocol::RequestId,
	query: jet_protocol::QueryRequest,
	timeout_ms: Option<u32>,
) -> ServerMessage {
	let Some(timeout_ms) = timeout_ms else {
		return answer(core, actor, minor, id, &query).await;
	};
	if minor < jet_protocol::EXECUTION_CONTROL_MINOR
		|| timeout_ms == 0
		|| timeout_ms > jet_protocol::MAX_QUERY_TIMEOUT_MS
	{
		return ServerMessage::Error {
			id: Some(id),
			error: wire_error(
				ErrorCategory::InvalidInput,
				"request.invalid_timeout",
				format!(
					"a Query bound must be 1 to {} milliseconds",
					jet_protocol::MAX_QUERY_TIMEOUT_MS
				),
			),
		};
	}
	match tokio::time::timeout(
		std::time::Duration::from_millis(timeout_ms.into()),
		answer(core, actor, minor, id, &query),
	)
	.await
	{
		Ok(reply) => reply,
		Err(_) => ServerMessage::Error {
			id: Some(id),
			error: wire_error(
				ErrorCategory::Unavailable,
				"request.timed_out",
				"the Query did not finish inside its bound".into(),
			),
		},
	}
}

async fn write_replies(
	mut writer: FrameWriter<OwnedWriteHalf>,
	mut replies: mpsc::Receiver<Frame>,
) {
	let mut queue = OutboundQueue::new(0);
	while let Some(frame) = replies.recv().await {
		if queue_reply(&mut queue, frame).is_err() {
			return;
		}
		for _ in 1..MAX_PENDING_REPLIES {
			let Ok(frame) = replies.try_recv() else {
				break;
			};
			if queue_reply(&mut queue, frame).is_err() {
				return;
			}
		}
		loop {
			match queue.write_next(&mut writer).await {
				Ok(true) => {}
				Ok(false) => break,
				Err(_) => return,
			}
		}
	}
}

fn queue_reply(
	queue: &mut OutboundQueue,
	frame: Frame,
) -> Result<(), jet_protocol::StreamQueueError> {
	let ordered = match &frame {
		Frame::Data { .. } => true,
		Frame::Control { payload, .. } => {
			matches!(
				decode_control::<StreamControl>(payload),
				Ok(StreamControl::TerminalGap { .. }
					| StreamControl::TerminalFinished { .. })
			) || matches!(
				decode_control::<ServerMessage>(payload),
				Ok(ServerMessage::TerminalAttached { .. }
					| ServerMessage::TerminalResized { .. })
			)
		}
	};
	if ordered {
		queue.queue_terminal_frame(frame)
	} else {
		queue.queue_control(frame)
	}
}
