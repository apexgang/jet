//! Interactive execution control, introduced in Jet protocol minor 19
//! (ADR-0083). Interrupting a turn and stopping a Run are separate requests
//! with separate outcomes; neither one is a transport-level cancellation
//! (ADR-0095).

use serde::{Deserialize, Serialize};

/// What an interactive control request asks of one managed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunControl {
	/// End the current turn and leave the Run able to accept the next one.
	InterruptTurn,
	/// End the whole execution, including its native processes.
	StopRun,
}

/// How far Jet had to go before the execution actually stopped. Everything
/// past `Interrupt` is a forced termination: the Harness was given the
/// chance to end its own work and did not take it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationStage {
	/// The Harness cancelled its own turn; no signal was sent and the Run
	/// remained available for later input.
	NativeCancellation,
	/// The native process ended after an interrupt signal.
	Interrupt,
	/// It ignored the interrupt and ended after a terminate signal.
	Terminate,
	/// It ignored both and was killed.
	Kill,
	/// Every signal was delivered and the execution still could not be
	/// observed ending; later work requires a new Run.
	Unobserved,
}

/// The exact terminal outcome of one control request, recorded once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTermination {
	/// The request this outcome answers.
	pub control: RunControl,
	/// The step that actually ended the work.
	pub stage: TerminationStage,
}
