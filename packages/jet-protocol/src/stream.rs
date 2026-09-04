//! Prioritized, credit-controlled stream scheduling.

use crate::stream_control::StreamControl;
use crate::{
	ErrorCategory, Frame, MAX_CONTROL_FRAME, MAX_DATA_FRAME, RecoveryAction,
	StreamId, WireError, encode_control,
};
use std::collections::{HashMap, VecDeque};

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

/// Failure while admitting or scheduling one stream frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamQueueError {
	/// A frame of the wrong kind was passed to a control queue.
	#[error("only control frames may enter a control queue")]
	ExpectedControl,
	/// Application streams must not use the reserved connection stream.
	#[error("the connection stream cannot carry application traffic")]
	ConnectionStream,
	/// A stream with this ID is already open.
	#[error("stream {0:?} is already open")]
	DuplicateStream(StreamId),
	/// The bounded per-connection stream registry is full.
	#[error("connection already has its maximum of {limit} binary streams")]
	TooManyStreams {
		/// Configured stream registry limit.
		limit: usize,
	},
	/// No binary stream with this ID is open.
	#[error("stream {0:?} is not open")]
	UnknownStream(StreamId),
	/// A stream cannot close while it still has queued data.
	#[error("stream {0:?} still has queued data")]
	StreamBusy(StreamId),
	/// Adding credit would overflow its counter.
	#[error("credit for stream {0:?} exceeds the supported range")]
	CreditOverflow(StreamId),
	/// A binary stream attempted to advance beyond receiver-issued credit.
	#[error(
		"stream {stream_id:?} has {available} bytes of credit but needs {requested}"
	)]
	InsufficientCredit {
		/// Stream that lacks credit.
		stream_id: StreamId,
		/// Remaining receiver-issued byte credit.
		available: u64,
		/// Bytes in the rejected chunk.
		requested: u64,
	},
	/// One raw chunk exceeded the protocol maximum.
	#[error("data chunk of {declared} bytes exceeds the {limit} byte limit")]
	OversizedData {
		/// Chunk size presented by the producer.
		declared: usize,
		/// Enforced protocol maximum.
		limit: usize,
	},
	/// A lossless stream must wait for queued bytes to drain.
	#[error(
		"stream {stream_id:?} is backpressured at {queued_bytes} of {limit} queued bytes"
	)]
	Backpressured {
		/// Lossless stream that must pause.
		stream_id: StreamId,
		/// Raw bytes already queued for the connection.
		queued_bytes: usize,
		/// Configured connection queue limit.
		limit: usize,
	},
	/// The bounded direct-control queue is full.
	#[error("control queue is full at its {limit} byte limit")]
	ControlBackpressured {
		/// Configured control queue limit.
		limit: usize,
	},
	/// Jet could not encode its own explicit stream-control report.
	#[error("stream control could not be encoded")]
	ControlEncoding,
	/// The semantic Event window is full and the connection must close.
	#[error(
		"slow Event consumer must reconnect and resume after cursor {resume_after}"
	)]
	SlowConsumer {
		/// Last Event cursor already delivered to the client.
		resume_after: u64,
	},
	/// Event cursors must be queued in Plane order.
	#[error("Event cursor {received} does not follow {previous}")]
	EventOutOfOrder {
		/// Last delivered or queued Event cursor.
		previous: u64,
		/// Cursor presented by the producer.
		received: u64,
	},
	/// A stream's byte offset cannot be represented.
	#[error("stream {0:?} byte offset overflowed")]
	OffsetOverflow(StreamId),
}

impl StreamQueueError {
	/// Returns the stable final error a writer sends before closing a slow
	/// Event consumer. Other queue failures are local backpressure and do not
	/// require disconnecting the connection.
	#[must_use]
	pub fn disconnect_error(&self) -> Option<WireError> {
		let Self::SlowConsumer { resume_after } = *self else {
			return None;
		};
		// ASVS 16.5.1 and 16.5.3: expose only a stable recovery cursor,
		// never native queue or transport details, and fail closed.
		Some(WireError {
			category: ErrorCategory::Unavailable,
			code: "protocol.slow_consumer".into(),
			retryable: true,
			message: "the Event consumer exceeded its bounded window; reconnect and replay after the supplied cursor".into(),
			revision_conflict: None,
			restart: None,
			recovery_actions: vec![RecoveryAction::ResumeEvents {
				after: resume_after,
			}],
		})
	}
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
	binary: VecDeque<Frame>,
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
			binary: VecDeque::new(),
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
	/// Rejects data frames, application traffic on stream zero, oversized
	/// frames, or a full bounded control queue.
	pub fn queue_control(
		&mut self,
		frame: Frame,
	) -> Result<(), StreamQueueError> {
		let Frame::Control {
			stream_id,
			ref payload,
		} = frame
		else {
			return Err(StreamQueueError::ExpectedControl);
		};
		if stream_id.is_connection() {
			return Err(StreamQueueError::ConnectionStream);
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
	pub fn next_frame(&mut self) -> Option<Frame> {
		if let Some(frame) = self.control.pop_front() {
			self.control_bytes -= frame.payload().len();
			return Some(frame);
		}
		if let Some(QueuedEvent { cursor, frame }) = self.events.pop_front() {
			self.event_bytes -= frame.payload().len();
			self.last_delivered_cursor = cursor;
			return Some(frame);
		}
		let frame = self.binary.pop_front()?;
		self.binary_bytes -= frame.payload().len();
		Some(frame)
	}
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
