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

struct Request {
	stream_id: StreamId,
	payload: Vec<u8>,
	message: ClientMessage,
}

enum Stop {
	Disconnected,
	Draining,
	Protocol(WireError),
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
				capacity
			),
		);
		let final_error = match stop {
			Stop::Disconnected => None,
			Stop::Draining => Some(draining_error()),
			Stop::Protocol(error) => Some(error),
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
			Ok(Frame::Data { .. }) => {
				return Stop::Protocol(wire_error(
					ErrorCategory::InvalidInput,
					"protocol.unexpected_data_frame",
					"no inbound binary stream is open on this connection"
						.into(),
				));
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
				return Stop::Protocol(wire_error(
					ErrorCategory::InvalidInput,
					"protocol.invalid_frame",
					"the frame violated the protocol limits".into(),
				));
			}
		};
		if minor >= MULTIPLEXED_STREAMS_MINOR && stream_id.is_connection() {
			return Stop::Protocol(wire_error(
				ErrorCategory::InvalidInput,
				"protocol.invalid_stream",
				"requests must use a numbered application stream".into(),
			));
		}
		let message = match decode_control(&payload) {
			Ok(message) => message,
			Err(_) => {
				return Stop::Protocol(match decode_control(&payload) {
					Ok(StreamControl::Credit { .. }) => wire_error(
						ErrorCategory::InvalidInput,
						"protocol.unknown_stream",
						"credit addressed a binary stream that is not open"
							.into(),
					),
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
				});
			}
		};
		if requests
			.send(Request {
				stream_id,
				payload,
				message,
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
) {
	while let Some(Request {
		stream_id,
		payload,
		message,
	}) = requests.recv().await
	{
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
		if replies
			.send(Frame::stream_control(stream_id, payload))
			.await
			.is_err()
		{
			return;
		}
		// An acknowledged Command may have committed an Effect; the work it
		// asks for begins once the acknowledgement is on its way and never
		// holds up this connection's next request (ADR-0064).
		if acknowledged {
			let effect_core = Arc::clone(&core);
			tokio::spawn(async move {
				if let Err(error) = effect_core.perform_promotions().await {
					eprintln!(
						"jetd: cannot record a Workspace promotion outcome: {error}"
					);
				}
			});
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
		ClientMessage::Query { id, query } => {
			answer(core, actor, minor, id, &query).await
		}
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

async fn write_replies(
	mut writer: FrameWriter<OwnedWriteHalf>,
	mut replies: mpsc::Receiver<Frame>,
) {
	let mut queue = OutboundQueue::new(0);
	while let Some(frame) = replies.recv().await {
		if queue.queue_control(frame).is_err() {
			return;
		}
		while let Ok(frame) = replies.try_recv() {
			if queue.queue_control(frame).is_err() {
				return;
			}
		}
		loop {
			match queue.write_next(&mut writer).await {
				Ok(true) => {}
				Ok(false) => break,
				Err(_) => return,
			}
			while let Ok(frame) = replies.try_recv() {
				if queue.queue_control(frame).is_err() {
					return;
				}
			}
		}
	}
}
