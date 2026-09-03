//! Length-prefixed frames carrying JSON control or raw binary data.
//!
//! Wire layout of one frame: one kind byte, a big-endian `u32` payload
//! length, then the payload. Readers validate the declared length against
//! the per-kind limit before allocating anything for the payload.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Upper bound of one JSON control frame payload (1 MiB).
pub const MAX_CONTROL_FRAME: usize = 1024 * 1024;
/// Upper bound of one raw binary data frame payload (256 KiB).
pub const MAX_DATA_FRAME: usize = 256 * 1024;

const HEADER_LEN: usize = 5;

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

/// One decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
	/// Strict JSON control payload.
	Control(Vec<u8>),
	/// Raw binary data payload.
	Data(Vec<u8>),
}

impl Frame {
	fn kind(&self) -> FrameKind {
		match self {
			Self::Control(_) => FrameKind::Control,
			Self::Data(_) => FrameKind::Data,
		}
	}

	fn payload(&self) -> &[u8] {
		match self {
			Self::Control(bytes) | Self::Data(bytes) => bytes,
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
	/// The peer closed the stream between frames.
	#[error("peer closed the connection")]
	Closed,
	/// Transport failure.
	#[error("transport error: {0}")]
	Io(#[from] std::io::Error),
}

impl PartialEq for FrameError {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(
				Self::Oversized {
					kind,
					declared,
					limit,
				},
				Self::Oversized {
					kind: other_kind,
					declared: other_declared,
					limit: other_limit,
				},
			) => {
				kind == other_kind
					&& declared == other_declared
					&& limit == other_limit
			}
			(Self::UnknownKind(a), Self::UnknownKind(b)) => a == b,
			(Self::Closed, Self::Closed) => true,
			(Self::Io(a), Self::Io(b)) => a.kind() == b.kind(),
			(
				Self::Oversized { .. }
				| Self::UnknownKind(_)
				| Self::Closed
				| Self::Io(_),
				_,
			) => false,
		}
	}
}

/// Reads bounded frames from an async byte stream.
#[derive(Debug)]
pub struct FrameReader<R> {
	inner: R,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
	/// Wraps a readable stream.
	pub fn new(inner: R) -> Self {
		Self { inner }
	}

	/// Reads the next frame, validating its declared size before allocation.
	///
	/// # Errors
	///
	/// Returns [`FrameError::Closed`] when the peer closed the stream at a
	/// frame boundary, and the other variants for malformed or oversized
	/// headers or transport failures.
	pub async fn read(&mut self) -> Result<Frame, FrameError> {
		let mut header = [0u8; HEADER_LEN];
		match self.inner.read_exact(&mut header).await {
			Ok(_) => {}
			Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
				return Err(FrameError::Closed);
			}
			Err(error) => return Err(error.into()),
		}
		let kind = FrameKind::from_byte(header[0])
			.ok_or(FrameError::UnknownKind(header[0]))?;
		let declared =
			u32::from_be_bytes([header[1], header[2], header[3], header[4]])
				as usize;
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
			FrameKind::Control => Frame::Control(payload),
			FrameKind::Data => Frame::Data(payload),
		})
	}
}

/// Writes bounded frames to an async byte stream.
#[derive(Debug)]
pub struct FrameWriter<W> {
	inner: W,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
	/// Wraps a writable stream.
	pub fn new(inner: W) -> Self {
		Self { inner }
	}

	/// Writes one frame and flushes it.
	///
	/// # Errors
	///
	/// Returns [`FrameError::Oversized`] without writing anything when the
	/// payload exceeds its kind's limit, or the transport failure otherwise.
	pub async fn write(&mut self, frame: &Frame) -> Result<(), FrameError> {
		let kind = frame.kind();
		let payload = frame.payload();
		if payload.len() > kind.limit() {
			return Err(FrameError::Oversized {
				kind,
				declared: payload.len(),
				limit: kind.limit(),
			});
		}
		let length = u32::try_from(payload.len()).map_err(|_| {
			FrameError::Oversized {
				kind,
				declared: payload.len(),
				limit: kind.limit(),
			}
		})?;
		let mut buffer = Vec::with_capacity(HEADER_LEN + payload.len());
		buffer.push(kind as u8);
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
