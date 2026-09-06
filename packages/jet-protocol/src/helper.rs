//! Owner-only Run-role helper protocol. Native bytes are opaque to jetfueld.
use crate::{ProtocolOffer, ProtocolVersion};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Immutable, host-written launch boundary for a Run-role helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperConfig {
	/// Authoritative Run identity.
	pub execution_id: Uuid,
	/// Canonical, revalidated working directory.
	pub working_directory: String,
	/// Executable disclosures from the accepted Craft specification.
	pub executables: Vec<String>,
}

/// Fresh helper connection handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperHello {
	/// The Run this connection belongs to.
	pub execution_id: Uuid,
	/// Independent helper protocol offer.
	pub protocol: ProtocolOffer,
}

/// Version selected before accepting native work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperReady {
	/// Negotiated helper version.
	pub version: ProtocolVersion,
	/// The helper's OS process identity.
	pub helper_pid: u32,
}

/// Craft requests at the generic helper boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelperCommand {
	/// Start exactly one native Harness under the host's fixed working root.
	Launch {
		/// One accepted executable disclosure.
		program: String,
		/// Argument vector; never shell source.
		arguments: Vec<String>,
		/// Initial native input written to standard input.
		input: String,
	},
	/// Release source records only after jetd committed their semantics.
	Acknowledge {
		/// End offset of the durably processed spool record.
		source_offset: u64,
	},
}

/// Native source record. The offset measures the end of its spool record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperRecord {
	/// Monotonically increasing source offset.
	pub source_offset: u64,
	/// Native process observation.
	pub event: HelperEvent,
}

/// Native process facts, independent from Craft interpretation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelperEvent {
	/// The OS definitively rejected launch; no Harness remains alive.
	LaunchFailed,
	/// The native process was spawned successfully.
	Started {
		/// OS process identity.
		harness_pid: u32,
	},
	/// A bounded chunk from one native output pipe.
	Output {
		/// Which pipe supplied the bytes.
		stream: NativeStream,
		/// Opaque bytes, bounded to 4096 bytes per record.
		bytes: Vec<u8>,
	},
	/// Both output pipes drained and the process was reaped.
	Exited {
		/// Exit code, or absent when terminated by a signal.
		exit_code: Option<i32>,
	},
}

/// Native pipe identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStream {
	/// Standard output.
	Stdout,
	/// Standard error.
	Stderr,
}
