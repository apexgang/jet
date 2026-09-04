//! Length-prefixed frames carrying JSON control or raw binary data.
//!
//! Wire layout of one frame: one kind byte, a big-endian `u32` stream ID, a
//! big-endian `u32` payload length, then the payload. Readers validate the
//! kind, stream, and declared length before allocating the payload.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Upper bound of one JSON control frame payload (1 MiB).
pub const MAX_CONTROL_FRAME: usize = 1024 * 1024;
/// Upper bound of one raw binary data frame payload (256 KiB).
pub const MAX_DATA_FRAME: usize = 256 * 1024;

const LEGACY_HEADER_LEN: usize = 5;
const STREAM_HEADER_LEN: usize = 9;

/// The typed number of one multiplexed stream on a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamId(u32);

impl StreamId {
	/// Creates an application stream ID. Zero is reserved for connection-level
	/// handshake and error control.
	#[must_use]
	pub const fn new(value: u32) -> Option<Self> {
		if value == 0 { None } else { Some(Self(value)) }
	}

	/// Returns the on-wire numeric value.
	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}

	/// Whether this is the reserved connection-level control stream.
	#[must_use]
	pub const fn is_connection(self) -> bool {
		self.0 == 0
	}

	const fn from_wire(value: u32) -> Self {
		Self(value)
	}
}

/// Reserved stream for handshake and connection-level control.
pub const CONNECTION_STREAM: StreamId = StreamId(0);

/// The kind byte that leads every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
	/// Strict JSON control payload.
	Control = 0,
	/// Raw binary data payload.
	Data = 1,
}

impl FrameKind {
	fn from_byte(byte: u8) -> Option<Self> {
		match byte {
			0 => Some(Self::Control),
			1 => Some(Self::Data),
			_ => None,
		}
	}

	fn limit(self) -> usize {
		match self {
			Self::Control => MAX_CONTROL_FRAME,
			Self::Data => MAX_DATA_FRAME,
		}
	}
}

/// Per-kind send limits, negotiated down from the protocol maxima during
/// the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
	/// Largest control frame payload that may be sent.
	pub control: usize,
	/// Largest data frame payload that may be sent.
	pub data: usize,
}

impl Default for FrameLimits {
	fn default() -> Self {
		Self {
			control: MAX_CONTROL_FRAME,
			data: MAX_DATA_FRAME,
		}
	}
}

impl FrameLimits {
	/// The limits both peers can honor: the smaller of each pair.
	#[must_use]
	pub fn negotiate(self, other: Self) -> Self {
		Self {
			control: self.control.min(other.control),
			data: self.data.min(other.data),
		}
	}

	fn for_kind(self, kind: FrameKind) -> usize {
		match kind {
			FrameKind::Control => self.control,
			FrameKind::Data => self.data,
		}
	}
}

/// One decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
	/// Strict JSON control payload.
	Control {
		/// Numbered logical stream carrying the message.
		stream_id: StreamId,
		/// Encoded JSON bytes.
		payload: Vec<u8>,
	},
	/// Raw binary data payload.
	Data {
		/// Numbered terminal or Artifact stream carrying the bytes.
		stream_id: StreamId,
		/// Unencoded terminal or Artifact bytes.
		payload: Vec<u8>,
	},
}

impl Frame {
	/// Creates connection-level control, including handshake messages.
	#[must_use]
	pub fn control(payload: Vec<u8>) -> Self {
		Self::Control {
			stream_id: CONNECTION_STREAM,
			payload,
		}
	}

	/// Creates control on the connection stream or a numbered Command, Query,
	/// Event, terminal, or Artifact stream.
	#[must_use]
	pub fn stream_control(stream_id: StreamId, payload: Vec<u8>) -> Self {
		Self::Control { stream_id, payload }
	}

	/// Creates raw binary data on a numbered terminal or Artifact stream.
	#[must_use]
	pub fn data(stream_id: StreamId, payload: Vec<u8>) -> Self {
		Self::Data { stream_id, payload }
	}

	/// Returns the numbered stream carrying this frame.
	#[must_use]
	pub fn stream_id(&self) -> StreamId {
		match self {
			Self::Control { stream_id, .. } | Self::Data { stream_id, .. } => {
				*stream_id
			}
		}
	}

	fn kind(&self) -> FrameKind {
		match self {
			Self::Control { .. } => FrameKind::Control,
			Self::Data { .. } => FrameKind::Data,
		}
	}

	/// Returns the JSON or raw binary payload within the protocol crate.
	#[must_use]
	pub(crate) fn payload(&self) -> &[u8] {
		match self {
			Self::Control { payload, .. } | Self::Data { payload, .. } => {
				payload
			}
		}
	}
}

/// Failure while reading or writing frames.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
	/// A frame declared a payload larger than its kind allows.
	#[error(
		"{kind:?} frame of {declared} bytes exceeds the {limit} byte limit"
	)]
	Oversized {
		/// Kind of the offending frame.
		kind: FrameKind,
		/// Declared payload length.
		declared: usize,
		/// Limit for that kind.
		limit: usize,
	},
	/// The kind byte is not defined by this protocol version.
	#[error("unknown frame kind {0}")]
	UnknownKind(u8),
	/// Raw binary data used the reserved connection-level control stream.
	#[error("{kind:?} frame cannot use stream {stream_id:?}")]
	InvalidStream {
		/// Kind of the offending frame.
		kind: FrameKind,
		/// Invalid stream ID from its envelope.
		stream_id: StreamId,
	},
	/// A numbered stream was used before stream multiplexing was negotiated.
	#[error("stream {0:?} used before multiplexing was negotiated")]
	MultiplexingDisabled(StreamId),
	/// The peer closed the stream between frames.
	#[error("peer closed the connection")]
	Closed,
	/// Transport failure.
	#[error("transport error: {0}")]
	Io(#[from] std::io::Error),
}

/// Reads bounded frames from an async byte stream.
#[derive(Debug)]
pub struct FrameReader<R> {
	inner: R,
	multiplexed: bool,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
	/// Wraps a readable stream.
	pub fn new(inner: R) -> Self {
		Self {
			inner,
			multiplexed: false,
		}
	}

	/// Switches from the legacy handshake envelope to the negotiated stream
	/// envelope. Both peers call this only after agreeing on the protocol
	/// minor that introduced multiplexing.
	pub fn enable_multiplexing(&mut self) {
		self.multiplexed = true;
	}

	/// Reads the next frame, validating its declared size before allocation.
	///
	/// # Errors
	///
	/// Returns [`FrameError::Closed`] when the peer closed the stream at a
	/// frame boundary, and the other variants for malformed or oversized
	/// headers or transport failures.
	pub async fn read(&mut self) -> Result<Frame, FrameError> {
		let mut kind = [0u8; 1];
		match self.inner.read_exact(&mut kind).await {
			Ok(_) => {}
			Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
				return Err(FrameError::Closed);
			}
			Err(error) => return Err(error.into()),
		}
		let kind = FrameKind::from_byte(kind[0])
			.ok_or(FrameError::UnknownKind(kind[0]))?;
		let (stream_id, declared) = if self.multiplexed {
			let mut envelope = [0u8; STREAM_HEADER_LEN - 1];
			self.inner.read_exact(&mut envelope).await?;
			(
				StreamId::from_wire(u32::from_be_bytes([
					envelope[0],
					envelope[1],
					envelope[2],
					envelope[3],
				])),
				u32::from_be_bytes([
					envelope[4],
					envelope[5],
					envelope[6],
					envelope[7],
				]) as usize,
			)
		} else {
			let mut length = [0u8; LEGACY_HEADER_LEN - 1];
			self.inner.read_exact(&mut length).await?;
			(CONNECTION_STREAM, u32::from_be_bytes(length) as usize)
		};
		// ASVS 1.5.2, 2.2.1, and 15.3.5: validate the envelope's
		// security-sensitive kind/stream combination before allocation.
		if self.multiplexed
			&& kind == FrameKind::Data
			&& stream_id.is_connection()
		{
			return Err(FrameError::InvalidStream { kind, stream_id });
		}
		if declared > kind.limit() {
			return Err(FrameError::Oversized {
				kind,
				declared,
				limit: kind.limit(),
			});
		}
		let mut payload = vec![0u8; declared];
		self.inner.read_exact(&mut payload).await?;
		Ok(match kind {
			FrameKind::Control => Frame::Control { stream_id, payload },
			FrameKind::Data => Frame::Data { stream_id, payload },
		})
	}
}

/// Writes bounded frames to an async byte stream.
#[derive(Debug)]
pub struct FrameWriter<W> {
	inner: W,
	limits: FrameLimits,
	multiplexed: bool,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
	/// Wraps a writable stream, sending up to the protocol maxima.
	pub fn new(inner: W) -> Self {
		Self {
			inner,
			limits: FrameLimits::default(),
			multiplexed: false,
		}
	}

	/// Applies the limits negotiated during the handshake.
	pub fn set_limits(&mut self, limits: FrameLimits) {
		self.limits = limits;
	}

	/// Switches subsequent frames to the negotiated stream envelope.
	pub fn enable_multiplexing(&mut self) {
		self.multiplexed = true;
	}

	/// Writes one frame and flushes it.
	///
	/// # Errors
	///
	/// Returns [`FrameError::Oversized`] without writing anything when the
	/// payload exceeds the negotiated limit for its kind, or the transport
	/// failure otherwise.
	pub async fn write(&mut self, frame: &Frame) -> Result<(), FrameError> {
		let kind = frame.kind();
		let stream_id = frame.stream_id();
		let payload = frame.payload();
		let limit = self.limits.for_kind(kind);
		if !self.multiplexed && !stream_id.is_connection() {
			return Err(FrameError::MultiplexingDisabled(stream_id));
		}
		if self.multiplexed
			&& kind == FrameKind::Data
			&& stream_id.is_connection()
		{
			return Err(FrameError::InvalidStream { kind, stream_id });
		}
		if payload.len() > limit {
			return Err(FrameError::Oversized {
				kind,
				declared: payload.len(),
				limit,
			});
		}
		let length = u32::try_from(payload.len()).map_err(|_| {
			FrameError::Oversized {
				kind,
				declared: payload.len(),
				limit,
			}
		})?;
		let header_len = if self.multiplexed {
			STREAM_HEADER_LEN
		} else {
			LEGACY_HEADER_LEN
		};
		let mut buffer = Vec::with_capacity(header_len + payload.len());
		buffer.push(kind as u8);
		if self.multiplexed {
			buffer.extend_from_slice(&stream_id.get().to_be_bytes());
		}
		buffer.extend_from_slice(&length.to_be_bytes());
		buffer.extend_from_slice(payload);
		self.inner.write_all(&buffer).await?;
		self.inner.flush().await?;
		Ok(())
	}
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
