//! Failures connecting to or communicating with a Plane.

use jet_protocol::{ControlError, FrameError, WireError};

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
