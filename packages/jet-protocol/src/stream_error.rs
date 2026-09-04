//! Failures produced by bounded stream admission and scheduling.

use crate::{ErrorCategory, RecoveryAction, StreamId, WireError};

/// Failure while admitting or scheduling one stream frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamQueueError {
	/// A frame of the wrong kind was passed to a control queue.
	#[error("only control frames may enter a control queue")]
	ExpectedControl,
	/// Empty JSON control cannot be a message and would evade byte quotas.
	#[error("control frames must not be empty")]
	EmptyControl,
	/// Empty binary frames carry no progress and would evade byte quotas.
	#[error("data frames must not be empty")]
	EmptyData,
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

#[cfg(test)]
#[path = "stream_error_tests.rs"]
mod tests;
