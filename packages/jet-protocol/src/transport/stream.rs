//! Prioritized, credit-controlled stream scheduling.

use crate::transport::stream_control::StreamControl;
pub use crate::transport::stream_error::StreamQueueError;
use crate::{
	Frame, FrameError, FrameWriter, MAX_CONTROL_FRAME, MAX_DATA_FRAME,
	StreamId, encode_control,
};
use std::collections::{HashMap, VecDeque};
use tokio::io::AsyncWrite;

/// Maximum semantic Events queued for one GUI connection (ADR-0081).
pub const MAX_EVENT_WINDOW_EVENTS: usize = 1_000;
/// Maximum encoded semantic Event bytes queued for one GUI connection.
pub const MAX_EVENT_WINDOW_BYTES: usize = 2 * 1024 * 1024;
/// Maximum pending non-Event control bytes on one connection.
pub const MAX_CONTROL_QUEUE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum pending terminal and Artifact bytes on one connection.
pub const MAX_BINARY_QUEUE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum simultaneously open binary streams on one connection.
pub const MAX_OPEN_BINARY_STREAMS: usize = 256;

/// Independently bounded outbound queues for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundLimits {
	/// Maximum queued direct-control payload bytes.
	pub control_bytes: usize,
	/// Maximum queued semantic Event count.
	pub event_count: usize,
	/// Maximum queued encoded semantic Event bytes.
	pub event_bytes: usize,
	/// Maximum queued raw binary payload bytes.
	pub binary_bytes: usize,
	/// Maximum simultaneously registered terminal and Artifact streams.
	pub open_streams: usize,
}

impl Default for OutboundLimits {
	fn default() -> Self {
		Self {
			control_bytes: MAX_CONTROL_QUEUE_BYTES,
			event_count: MAX_EVENT_WINDOW_EVENTS,
			event_bytes: MAX_EVENT_WINDOW_BYTES,
			binary_bytes: MAX_BINARY_QUEUE_BYTES,
			open_streams: MAX_OPEN_BINARY_STREAMS,
		}
	}
}

/// Whether a raw binary stream may report loss or must backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryStreamKind {
	/// Rolling terminal output may drop bytes only with an explicit gap.
	Terminal,
	/// Artifact bytes are lossless and backpressure when their queue is full.
	Artifact,
}

#[derive(Debug, Clone, Copy)]
struct BinaryStream {
	kind: BinaryStreamKind,
	credit: u64,
	next_offset: u64,
}

#[derive(Debug)]
struct QueuedEvent {
	cursor: u64,
	frame: Frame,
}

/// Result of admitting raw data to an outbound stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataQueueOutcome {
	/// The bytes were queued and consumed receiver-issued credit.
	Queued,
	/// Terminal bytes were dropped because the bounded data queue was full.
	TerminalGap {
		/// Offset of the first omitted terminal byte.
		first_missing_offset: u64,
		/// Number of omitted terminal bytes.
		missing_bytes: u64,
	},
}

/// Bounded outbound queues with strict control-before-bulk scheduling.
#[derive(Debug)]
pub struct OutboundQueue {
	limits: OutboundLimits,
	control: VecDeque<Frame>,
	control_bytes: usize,
	events: VecDeque<QueuedEvent>,
	event_bytes: usize,
	last_delivered_cursor: u64,
	last_queued_cursor: u64,
	event_awaiting_write: Option<u64>,
	binary: VecDeque<Frame>,
	ordered: VecDeque<Frame>,
	binary_bytes: usize,
	streams: HashMap<StreamId, BinaryStream>,
}

impl OutboundQueue {
	/// Creates a queue using the protocol's production bounds.
	#[must_use]
	pub fn new(last_delivered_cursor: u64) -> Self {
		Self::with_limits(last_delivered_cursor, OutboundLimits::default())
	}

	/// Creates a queue with explicit lower bounds, useful for constrained
	/// connections and deterministic backpressure tests.
	#[must_use]
	pub fn with_limits(
		last_delivered_cursor: u64,
		limits: OutboundLimits,
	) -> Self {
		Self {
			limits,
			control: VecDeque::new(),
			control_bytes: 0,
			events: VecDeque::new(),
			event_bytes: 0,
			last_delivered_cursor,
			last_queued_cursor: last_delivered_cursor,
			event_awaiting_write: None,
			binary: VecDeque::new(),
			ordered: VecDeque::new(),
			binary_bytes: 0,
			streams: HashMap::new(),
		}
	}

	/// Registers one numbered binary stream.
	///
	/// # Errors
	///
	/// Returns [`StreamQueueError::ConnectionStream`] for stream zero or
	/// [`StreamQueueError::DuplicateStream`] for an ID already in use.
	pub fn open_binary(
		&mut self,
		stream_id: StreamId,
		kind: BinaryStreamKind,
	) -> Result<(), StreamQueueError> {
		if stream_id.is_connection() {
			return Err(StreamQueueError::ConnectionStream);
		}
		if self.streams.contains_key(&stream_id) {
			return Err(StreamQueueError::DuplicateStream(stream_id));
		}
		if self.streams.len() >= self.limits.open_streams {
			return Err(StreamQueueError::TooManyStreams {
				limit: self.limits.open_streams,
			});
		}
		self.streams.insert(
			stream_id,
			BinaryStream {
				kind,
				credit: 0,
				next_offset: 0,
			},
		);
		Ok(())
	}

	/// Removes an idle binary stream so its ID and registry capacity may be
	/// reused.
	///
	/// # Errors
	///
	/// Returns [`StreamQueueError::UnknownStream`] for an unopened ID or
	/// [`StreamQueueError::StreamBusy`] while that stream still has data in
	/// the outbound queue.
	pub fn close_binary(
		&mut self,
		stream_id: StreamId,
	) -> Result<(), StreamQueueError> {
		if !self.streams.contains_key(&stream_id) {
			return Err(StreamQueueError::UnknownStream(stream_id));
		}
		if self
			.binary
			.iter()
			.any(|frame| frame.stream_id() == stream_id)
			|| self
				.control
				.iter()
				.any(|frame| frame.stream_id() == stream_id)
			|| self
				.events
				.iter()
				.any(|event| event.frame.stream_id() == stream_id)
		{
			return Err(StreamQueueError::StreamBusy(stream_id));
		}
		self.streams.remove(&stream_id);
		Ok(())
	}

	/// Adds receiver-issued byte credit to a binary stream.
	///
	/// # Errors
	///
	/// Returns [`StreamQueueError::UnknownStream`] or rejects arithmetic
	/// overflow without changing the available credit.
	pub fn grant_credit(
		&mut self,
		stream_id: StreamId,
		bytes: u64,
	) -> Result<(), StreamQueueError> {
		let stream = self
			.streams
			.get_mut(&stream_id)
			.ok_or(StreamQueueError::UnknownStream(stream_id))?;
		stream.credit = stream
			.credit
			.checked_add(bytes)
			.ok_or(StreamQueueError::CreditOverflow(stream_id))?;
		Ok(())
	}

	/// Queues one non-Event control frame.
	///
	/// # Errors
	///
	/// Rejects data frames, oversized frames, or a full bounded control
	/// queue. Connection-level errors and legacy-minor replies may use stream
	/// zero.
	pub fn queue_control(
		&mut self,
		frame: Frame,
	) -> Result<(), StreamQueueError> {
		let Frame::Control {
			stream_id: _,
			ref payload,
		} = frame
		else {
			return Err(StreamQueueError::ExpectedControl);
		};
		if payload.is_empty() {
			return Err(StreamQueueError::EmptyControl);
		}
		let next = self.control_bytes.saturating_add(payload.len());
		if payload.len() > MAX_CONTROL_FRAME || next > self.limits.control_bytes
		{
			return Err(StreamQueueError::ControlBackpressured {
				limit: self.limits.control_bytes,
			});
		}
		self.control_bytes = next;
		self.control.push_back(frame);
		Ok(())
	}

	/// Queues one semantic Event without ever dropping it.
	///
	/// # Errors
	///
	/// Returns [`StreamQueueError::SlowConsumer`] with the last delivered
	/// cursor when either Event-window bound would be crossed. The connection
	/// must then close and resume through snapshot/replay (ADR-0081).
	pub fn queue_event(
		&mut self,
		cursor: u64,
		frame: Frame,
	) -> Result<(), StreamQueueError> {
		let Frame::Control {
			stream_id,
			ref payload,
		} = frame
		else {
			return Err(StreamQueueError::ExpectedControl);
		};
		if payload.is_empty() {
			return Err(StreamQueueError::EmptyControl);
		}
		if stream_id.is_connection() {
			return Err(StreamQueueError::ConnectionStream);
		}
		if cursor <= self.last_queued_cursor {
			return Err(StreamQueueError::EventOutOfOrder {
				previous: self.last_queued_cursor,
				received: cursor,
			});
		}
		let next_bytes = self.event_bytes.saturating_add(payload.len());
		// ASVS 2.3.1, 2.3.2, 15.2.2, and 15.4.4: preserve Plane
		// order, enforce both documented bounds, and fail explicitly instead
		// of letting Event or binary pressure starve control traffic.
		if payload.len() > MAX_CONTROL_FRAME
			|| self.events.len() >= self.limits.event_count
			|| next_bytes > self.limits.event_bytes
		{
			return Err(StreamQueueError::SlowConsumer {
				resume_after: self.last_delivered_cursor,
			});
		}
		self.event_bytes = next_bytes;
		self.last_queued_cursor = cursor;
		self.events.push_back(QueuedEvent { cursor, frame });
		Ok(())
	}

	/// Queues one raw terminal or Artifact chunk.
	///
	/// # Errors
	///
	/// Rejects chunks over 256 KiB or beyond receiver credit. A full queue
	/// backpressures Artifacts and returns an explicit terminal gap for lossy
	/// terminal output.
	pub fn queue_data(
		&mut self,
		stream_id: StreamId,
		payload: Vec<u8>,
	) -> Result<DataQueueOutcome, StreamQueueError> {
		if payload.len() > MAX_DATA_FRAME {
			return Err(StreamQueueError::OversizedData {
				declared: payload.len(),
				limit: MAX_DATA_FRAME,
			});
		}
		if payload.is_empty() {
			return Err(StreamQueueError::EmptyData);
		}
		let requested = u64::try_from(payload.len())
			.map_err(|_| StreamQueueError::OffsetOverflow(stream_id))?;
		let stream = *self
			.streams
			.get(&stream_id)
			.ok_or(StreamQueueError::UnknownStream(stream_id))?;
		if requested > stream.credit {
			return Err(StreamQueueError::InsufficientCredit {
				stream_id,
				available: stream.credit,
				requested,
			});
		}
		let next_offset = stream
			.next_offset
			.checked_add(requested)
			.ok_or(StreamQueueError::OffsetOverflow(stream_id))?;
		let next_queued = self.binary_bytes.saturating_add(payload.len());
		if next_queued > self.limits.binary_bytes {
			return match stream.kind {
				BinaryStreamKind::Terminal => {
					let first_missing_offset = stream.next_offset;
					let gap = encode_control(&StreamControl::TerminalGap {
						first_missing_offset,
						missing_bytes: requested,
					})
					.map_err(|_| StreamQueueError::ControlEncoding)?;
					// The gap enters the independent control queue before the
					// source offset advances, so terminal loss is never silent.
					self.queue_control(Frame::stream_control(stream_id, gap))?;
					self.streams
						.get_mut(&stream_id)
						.expect("the stream remains open")
						.next_offset = next_offset;
					Ok(DataQueueOutcome::TerminalGap {
						first_missing_offset,
						missing_bytes: requested,
					})
				}
				BinaryStreamKind::Artifact => {
					Err(StreamQueueError::Backpressured {
						stream_id,
						queued_bytes: self.binary_bytes,
						limit: self.limits.binary_bytes,
					})
				}
			};
		}
		let stream = self
			.streams
			.get_mut(&stream_id)
			.expect("the stream remains open");
		stream.credit -= requested;
		stream.next_offset = next_offset;
		self.binary_bytes = next_queued;
		self.binary.push_back(Frame::data(stream_id, payload));
		Ok(DataQueueOutcome::Queued)
	}

	/// Removes the next frame, always choosing direct control, then semantic
	/// Events, before any raw binary data.
	///
	/// After the returned frame is written successfully, the caller must call
	/// [`Self::confirm_written`]. Until then no later frame is returned. This
	/// keeps an Event out of the recoverable cursor until transport delivery
	/// has succeeded.
	pub(crate) fn next_frame(&mut self) -> Option<Frame> {
		if self.event_awaiting_write.is_some() {
			return None;
		}
		if let Some(frame) = self.control.pop_front() {
			self.control_bytes -= frame.payload().len();
			return Some(frame);
		}
		if let Some(QueuedEvent { cursor, frame }) = self.events.pop_front() {
			self.event_bytes -= frame.payload().len();
			self.event_awaiting_write = Some(cursor);
			return Some(frame);
		}
		if let Some(frame) = self.ordered.pop_front() {
			match frame {
				Frame::Data { ref payload, .. } => {
					self.binary_bytes -= payload.len()
				}
				Frame::Control { ref payload, .. } => {
					self.control_bytes -= payload.len()
				}
			}
			return Some(frame);
		}
		let frame = self.binary.pop_front()?;
		self.binary_bytes -= frame.payload().len();
		Some(frame)
	}

	/// Confirms that the last frame returned by [`Self::next_frame`] reached
	/// the transport successfully. For an Event, this advances the cursor
	/// advertised if the connection later disconnects a slow consumer.
	pub(crate) fn confirm_written(&mut self) {
		if let Some(cursor) = self.event_awaiting_write.take() {
			self.last_delivered_cursor = cursor;
		}
	}

	/// Writes one scheduled frame and records Event delivery only after the
	/// transport flush succeeds.
	///
	/// # Errors
	///
	/// Returns the framing or transport error without advancing the
	/// recoverable Event cursor.
	pub async fn write_next<W: AsyncWrite + Unpin>(
		&mut self,
		writer: &mut FrameWriter<W>,
	) -> Result<bool, FrameError> {
		let Some(frame) = self.next_frame() else {
			return Ok(false);
		};
		writer.write(&frame).await?;
		self.confirm_written();
		Ok(true)
	}
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;

	use super::{
		BinaryStreamKind, DataQueueOutcome, OutboundLimits, OutboundQueue,
		StreamQueueError,
	};
	use crate::{
		ControlError, ErrorCategory, Frame, FrameReader, FrameWriter,
		RecoveryAction, ServerMessage, StreamControl, StreamId, WireError,
		decode_control, encode_control,
	};
	use tokio::io::duplex;

	fn stream(value: u32) -> StreamId {
		StreamId::new(value).unwrap()
	}

	#[test]
	fn control_and_events_are_sent_before_credit_controlled_binary_data() {
		let mut queue = OutboundQueue::new(40);
		queue
			.open_binary(stream(7), BinaryStreamKind::Artifact)
			.unwrap();
		queue.grant_credit(stream(7), 3).unwrap();
		assert_eq!(
			queue.queue_data(stream(7), vec![1, 2, 3]).unwrap(),
			DataQueueOutcome::Queued
		);
		let bulk = Frame::data(stream(7), vec![1, 2, 3]);
		let event =
			Frame::stream_control(stream(5), br#"{"kind":"event"}"#.to_vec());
		queue.queue_event(41, event.clone()).unwrap();
		let reply =
			Frame::stream_control(stream(3), br#"{"kind":"reply"}"#.to_vec());
		queue.queue_control(reply.clone()).unwrap();

		assert_eq!(queue.next_frame(), Some(reply));
		queue.confirm_written();
		assert_eq!(queue.next_frame(), Some(event));
		queue.confirm_written();
		assert_eq!(queue.next_frame(), Some(bulk));
	}

	#[test]
	fn binary_streams_cannot_advance_past_receiver_credit() {
		let mut queue = OutboundQueue::new(0);
		queue
			.open_binary(stream(1), BinaryStreamKind::Artifact)
			.unwrap();

		assert_eq!(
			queue.queue_data(stream(1), vec![1]).unwrap_err(),
			StreamQueueError::InsufficientCredit {
				stream_id: stream(1),
				available: 0,
				requested: 1,
			}
		);
		assert_eq!(queue.next_frame(), None);
	}

	#[test]
	fn empty_frames_cannot_grow_any_outbound_queue() {
		let mut queue = OutboundQueue::new(0);
		queue
			.open_binary(stream(1), BinaryStreamKind::Artifact)
			.unwrap();

		assert_eq!(
			queue
				.queue_control(Frame::stream_control(stream(2), vec![]))
				.unwrap_err(),
			StreamQueueError::EmptyControl
		);
		assert_eq!(
			queue
				.queue_event(1, Frame::stream_control(stream(3), vec![]))
				.unwrap_err(),
			StreamQueueError::EmptyControl
		);
		assert_eq!(
			queue.queue_data(stream(1), vec![]).unwrap_err(),
			StreamQueueError::EmptyData
		);
	}

	#[test]
	fn open_binary_streams_are_bounded_and_ids_can_be_reused_after_close() {
		let limits = OutboundLimits {
			open_streams: 1,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(0, limits);
		queue
			.open_binary(stream(1), BinaryStreamKind::Terminal)
			.unwrap();

		assert_eq!(
			queue
				.open_binary(stream(2), BinaryStreamKind::Artifact)
				.unwrap_err(),
			StreamQueueError::TooManyStreams { limit: 1 }
		);
		queue.close_binary(stream(1)).unwrap();
		queue
			.open_binary(stream(2), BinaryStreamKind::Artifact)
			.unwrap();
	}

	#[test]
	fn bounded_binary_queue_reports_terminal_gaps_but_backpressures_artifacts()
	{
		let limits = OutboundLimits {
			binary_bytes: 3,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(0, limits);
		queue
			.open_binary(stream(1), BinaryStreamKind::Artifact)
			.unwrap();
		queue
			.open_binary(stream(2), BinaryStreamKind::Terminal)
			.unwrap();
		queue.grant_credit(stream(1), 8).unwrap();
		queue.grant_credit(stream(2), 8).unwrap();
		queue.queue_data(stream(1), vec![1, 2, 3]).unwrap();

		assert_eq!(
			queue.queue_data(stream(2), vec![4, 5]).unwrap(),
			DataQueueOutcome::TerminalGap {
				first_missing_offset: 0,
				missing_bytes: 2,
			}
		);
		assert_eq!(
			queue.queue_data(stream(1), vec![6]).unwrap_err(),
			StreamQueueError::Backpressured {
				stream_id: stream(1),
				queued_bytes: 3,
				limit: 3,
			}
		);

		let gap = queue.next_frame().unwrap();
		assert_eq!(gap.stream_id(), stream(2));
		assert_eq!(
			decode_control::<StreamControl>(gap.payload()).unwrap(),
			StreamControl::TerminalGap {
				first_missing_offset: 0,
				missing_bytes: 2,
			}
		);
		assert_eq!(
			queue.next_frame(),
			Some(Frame::data(stream(1), vec![1, 2, 3]))
		);
		assert_eq!(
			queue.queue_data(stream(2), vec![6, 7]).unwrap(),
			DataQueueOutcome::Queued
		);
	}

	#[test]
	fn a_stream_id_cannot_be_reused_until_its_gap_control_is_written() {
		let limits = OutboundLimits {
			binary_bytes: 0,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(0, limits);
		queue
			.open_binary(stream(1), BinaryStreamKind::Terminal)
			.unwrap();
		queue.grant_credit(stream(1), 1).unwrap();
		assert!(matches!(
			queue.queue_data(stream(1), vec![1]).unwrap(),
			DataQueueOutcome::TerminalGap { .. }
		));

		assert_eq!(
			queue.close_binary(stream(1)).unwrap_err(),
			StreamQueueError::StreamBusy(stream(1))
		);
		assert!(queue.next_frame().is_some());
		queue.confirm_written();
		queue.close_binary(stream(1)).unwrap();
		queue
			.open_binary(stream(1), BinaryStreamKind::Artifact)
			.unwrap();
	}

	#[test]
	fn full_event_window_disconnects_with_the_last_delivered_cursor() {
		let limits = OutboundLimits {
			event_count: 1,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(40, limits);
		queue
			.queue_event(41, Frame::stream_control(stream(5), vec![1]))
			.unwrap();
		assert!(queue.next_frame().is_some());
		queue.confirm_written();
		queue
			.queue_event(42, Frame::stream_control(stream(5), vec![2]))
			.unwrap();

		let error = queue
			.queue_event(43, Frame::stream_control(stream(5), vec![3]))
			.unwrap_err();
		assert_eq!(error, StreamQueueError::SlowConsumer { resume_after: 41 });
	}

	#[test]
	fn an_event_is_not_recoverable_until_its_frame_write_succeeds() {
		let limits = OutboundLimits {
			event_count: 1,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(40, limits);
		queue
			.queue_event(41, Frame::stream_control(stream(5), vec![1]))
			.unwrap();
		assert!(queue.next_frame().is_some());
		queue
			.queue_event(42, Frame::stream_control(stream(5), vec![2]))
			.unwrap();

		assert_eq!(
			queue
				.queue_event(43, Frame::stream_control(stream(5), vec![3]))
				.unwrap_err(),
			StreamQueueError::SlowConsumer { resume_after: 40 }
		);
	}

	#[test]
	fn event_cursors_must_advance_strictly() {
		let mut queue = OutboundQueue::new(40);
		queue
			.queue_event(41, Frame::stream_control(stream(5), vec![1]))
			.unwrap();

		assert_eq!(
			queue
				.queue_event(41, Frame::stream_control(stream(5), vec![2]))
				.unwrap_err(),
			StreamQueueError::EventOutOfOrder {
				previous: 41,
				received: 41,
			}
		);
	}

	#[test]
	fn stream_control_allows_optional_fields_but_rejects_unknown_variants() {
		let credit: StreamControl = decode_control(
			br#"{"type":"credit","bytes":8,"future_optional":true}"#,
		)
		.unwrap();
		assert_eq!(credit, StreamControl::Credit { bytes: 8 });

		let unknown = decode_control::<StreamControl>(
			br#"{"type":"replace_artifact","bytes":8}"#,
		)
		.unwrap_err();
		assert!(matches!(unknown, ControlError::Malformed(_)), "{unknown:?}");
	}

	#[test]
	fn terminal_gap_has_an_exact_explicit_wire_shape() {
		let gap = StreamControl::TerminalGap {
			first_missing_offset: 9,
			missing_bytes: 4,
		};

		assert_eq!(
			String::from_utf8(encode_control(&gap).unwrap()).unwrap(),
			r#"{"type":"terminal_gap","first_missing_offset":"9","missing_bytes":"4"}"#
		);
	}

	#[tokio::test]
	async fn terminal_gap_control_precedes_already_queued_artifact_bytes_on_wire()
	 {
		let limits = OutboundLimits {
			binary_bytes: 3,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(0, limits);
		queue
			.open_binary(stream(1), BinaryStreamKind::Artifact)
			.unwrap();
		queue
			.open_binary(stream(2), BinaryStreamKind::Terminal)
			.unwrap();
		queue.grant_credit(stream(1), 3).unwrap();
		queue.grant_credit(stream(2), 2).unwrap();
		queue.queue_data(stream(1), vec![1, 2, 3]).unwrap();
		queue.queue_data(stream(2), vec![4, 5]).unwrap();
		let (client, server) = duplex(128);
		let mut writer = FrameWriter::new(client);
		let mut reader = FrameReader::new(server);
		writer.enable_multiplexing();
		reader.enable_multiplexing();

		assert!(queue.write_next(&mut writer).await.unwrap());
		let Frame::Control { stream_id, payload } =
			reader.read().await.unwrap()
		else {
			panic!("expected explicit terminal-gap control");
		};
		assert_eq!(
			(
				stream_id,
				decode_control::<StreamControl>(&payload).unwrap()
			),
			(
				stream(2),
				StreamControl::TerminalGap {
					first_missing_offset: 0,
					missing_bytes: 2,
				}
			)
		);
		assert!(queue.write_next(&mut writer).await.unwrap());
		assert_eq!(
			reader.read().await.unwrap(),
			Frame::data(stream(1), vec![1, 2, 3])
		);
	}

	#[tokio::test]
	async fn slow_consumer_error_is_sent_before_pending_event_with_written_cursor()
	 {
		let limits = OutboundLimits {
			event_count: 1,
			..OutboundLimits::default()
		};
		let mut queue = OutboundQueue::with_limits(40, limits);
		let (client, server) = duplex(512);
		let mut writer = FrameWriter::new(client);
		let mut reader = FrameReader::new(server);
		writer.enable_multiplexing();
		reader.enable_multiplexing();
		queue
			.queue_event(
				41,
				Frame::stream_control(
					stream(5),
					br#"{"kind":"event"}"#.to_vec(),
				),
			)
			.unwrap();
		assert!(queue.write_next(&mut writer).await.unwrap());
		assert!(matches!(
			reader.read().await.unwrap(),
			Frame::Control { .. }
		));
		queue
			.queue_event(
				42,
				Frame::stream_control(
					stream(5),
					br#"{"kind":"event"}"#.to_vec(),
				),
			)
			.unwrap();
		let error = queue
			.queue_event(
				43,
				Frame::stream_control(
					stream(5),
					br#"{"kind":"event"}"#.to_vec(),
				),
			)
			.unwrap_err()
			.disconnect_error()
			.unwrap();
		queue
			.queue_control(Frame::control(
				encode_control(&ServerMessage::Error { id: None, error })
					.unwrap(),
			))
			.unwrap();

		assert!(queue.write_next(&mut writer).await.unwrap());
		let Frame::Control { stream_id, payload } =
			reader.read().await.unwrap()
		else {
			panic!("expected a connection-level slow-consumer error");
		};
		assert_eq!(stream_id, crate::CONNECTION_STREAM);
		assert_eq!(
			decode_control::<ServerMessage>(&payload).unwrap(),
			ServerMessage::Error {
				id: None,
				error: WireError {
					category: ErrorCategory::Unavailable,
					code: "protocol.slow_consumer".into(),
					retryable: true,
					message: "the Event consumer exceeded its bounded window; reconnect and replay after the supplied cursor".into(),
					revision_conflict: None,
					restart: None,
					recovery_actions: vec![RecoveryAction::ResumeEvents {
						after: 41,
					}],
				},
			}
		);
	}
}

#[path = "terminal_queue.rs"]
mod terminal_queue;
