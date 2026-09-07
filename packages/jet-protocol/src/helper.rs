//! Owner-only Run-role helper protocol. Native bytes are opaque to jetfueld.
use crate::{ProtocolOffer, ProtocolVersion};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Immutable, host-written launch boundary for a Run-role helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperConfig {
	/// Authoritative Run identity.
	pub execution_id: Uuid,
	/// Canonical, revalidated working directory.
	pub working_directory: String,
	/// Executable disclosures from the accepted Craft specification.
	pub executables: Vec<String>,
	/// Registered Project root accepted by the host.
	pub project_directory: String,
	/// Exact Craft digest pinned for the Run.
	pub craft_digest: String,
}

/// Atomic non-secret identity published by a Run-role helper (Helper 1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperDescriptor {
	/// Atomically published source position; live handshakes refresh it.
	pub replay: HelperReplay,
	/// Execution role; this build supports only `run`.
	pub role: String,
	/// Host-provisioned execution boundaries.
	pub config: HelperConfig,
	/// Fresh instance identity, never reused by another helper.
	pub instance: Uuid,
	/// Helper PID.
	pub pid: u32,
	/// OS-observed process start and executable name.
	pub process_start: String,
	/// Deployed helper product version.
	pub version: String,
	/// SHA-256 of the helper executable at startup.
	pub sha256: String,
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
	/// Live helper identity, checked against the owner-only descriptor.
	pub descriptor: HelperDescriptor,
}

/// Whether a launched Harness keeps reading after its initial input.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum NativeInputMode {
	/// Close standard input once the initial input is written, so a Harness
	/// that reads to end of input proceeds. Decoding a launch that names no
	/// mode selects this, which is what every Craft had before Helper 1.3.
	#[default]
	Sealed,
	/// Keep standard input open for later `Input` until `CloseInput`
	/// (Helper 1.3). A Harness driven by a bidirectional native protocol
	/// takes its later turns, approval answers, and cancellations there.
	Streaming,
}

/// Craft requests at the generic helper boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelperCommand {
	/// Stop only this exact helper instance after interactive authorization.
	Terminate {
		/// Identity the user inspected.
		instance: Uuid,
	},
	/// Deliver one signal to the native process group without ending this
	/// helper or releasing its retained source (Helper 1.2). The native
	/// exit then reaches jetd through the ordinary spool, so partial
	/// output survives the stop (ADR-0083).
	Signal {
		/// Identity the caller validated before asking.
		instance: Uuid,
		/// Which signal to deliver.
		signal: NativeSignal,
	},
	/// Read-only handshake completed; leave execution and source untouched.
	Inspect,
	/// Reattach after validating the source boundary; never launches a process.
	Recover {
		/// Source boundary committed by jetd.
		source_offset: u64,
	},
	/// Start exactly one native Harness under the host's fixed working root.
	Launch {
		/// One accepted executable disclosure.
		program: String,
		/// Argument vector; never shell source.
		arguments: Vec<String>,
		/// Initial native input written to standard input.
		input: String,
		/// Whether this Harness accepts later `Input` (Helper 1.3).
		#[serde(default)]
		input_mode: NativeInputMode,
	},
	/// Write more native input to a Harness launched as `Streaming`
	/// (Helper 1.3). The bytes stay opaque to the helper, which neither
	/// frames nor interprets them.
	Input {
		/// Native input written verbatim to standard input.
		text: String,
	},
	/// Close a `Streaming` Harness's standard input (Helper 1.3). A Harness
	/// that ends on end of input stops here. No signal is delivered and no
	/// retained source is released, so its exit reaches the host as usual.
	CloseInput,
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

/// The escalation ladder jetd may ask a helper to deliver. Each step is an
/// explicit request; the helper never escalates on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSignal {
	/// Interrupt, which a Harness may handle and shut down cleanly.
	Interrupt,
	/// Terminate, which it may still handle.
	Terminate,
	/// Kill, which it cannot.
	Kill,
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

/// Confirmation that the signal was delivered to the live native process
/// group. Delivery is not an exit: the exit arrives as a source record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperSignalled {
	/// The helper instance that delivered it.
	pub instance: Uuid,
	/// The signal it delivered.
	pub signal: NativeSignal,
}

/// Confirmation sent only after the native child is proven stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperTerminated {
	/// The helper instance that stopped its execution.
	pub instance: Uuid,
}

/// Source boundaries retained by a helper. Only exact record boundaries resume.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperReplay {
	/// Last released boundary.
	pub acknowledged: u64,
	/// End of the newest retained record.
	pub produced: u64,
	/// The first unacknowledged record, if any.
	pub next_offset: Option<u64>,
}
