//! Execution port: jetd translates Craft/helper traffic into these domain observations.
use crate::{
	CoreError, ForkLaunchSource, RunId, run_command::LaunchPlan,
	run_craft::PinnedCraft, run_state::Observation,
};
use std::{future::Future, path::PathBuf, pin::Pin};

/// An asynchronous Adapter operation that can move across runtime workers.
pub type RunFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Host-specific Craft validation and native transport, supplied by jetd.
pub trait RunHost: std::fmt::Debug + Send + Sync {
	/// Resolves an installed identity and pins its accepted execution contract.
	fn pin(
		&self,
		home: PathBuf,
		id: String,
	) -> RunFuture<'_, Result<PinnedCraft, CoreError>>;
	/// Reads the Harness identity from an accepted Craft contract. Hosts that
	/// cannot interpret it must refuse cross-Harness continuation.
	fn harness(&self, _craft: &PinnedCraft) -> Result<String, CoreError> {
		Err(CoreError::conflict(
			"handoff.unsupported",
			"this Run host cannot validate Handoffs",
		))
	}
	/// Refreshes metadata for a new Run without selecting a different artifact.
	fn prepare_next_run(
		&self,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>>;
	/// Selects native fork delivery only when both pinned Harness contracts
	/// and the durable source identity are compatible. The safe default keeps
	/// the portable provenance package selected by Core.
	fn prepare_fork(
		&self,
		plan: LaunchPlan,
		_source: Option<ForkLaunchSource>,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(async move { Ok(plan) })
	}
	/// Starts the accepted execution, distinguishing rejection from uncertainty.
	fn start(
		&self,
		home: PathBuf,
		run_id: RunId,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>>;
	/// Enumerates owner-provisioned helper identities without acknowledging source.
	fn discover(
		&self,
		_home: PathBuf,
	) -> RunFuture<'_, Result<Vec<RunId>, CoreError>> {
		Box::pin(async { Ok(vec![]) })
	}
	/// Validates and reconnects an existing execution, never launching native work.
	fn recover(
		&self,
		_home: PathBuf,
		_run_id: RunId,
		_plan: LaunchPlan,
		_cursor: RunRecoveryCursor,
	) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunRecoveryError>> {
		Box::pin(async { Err(RunRecoveryError::Unsafe) })
	}

	/// Probes process liveness without adopting or acknowledging an execution.
	/// Only proven death returns Gone; ambiguous evidence remains Unsafe.
	fn probe(
		&self,
		_home: PathBuf,
		_id: RunId,
		_accepted: Option<LaunchPlan>,
		_helper_pid: Option<u32>,
	) -> RunFuture<'_, Result<(), RunRecoveryError>> {
		Box::pin(async { Err(RunRecoveryError::Unsafe) })
	}
	/// Read-only owner metadata; it does not authorize adoption or acknowledgement.
	fn describe(
		&self,
		_home: PathBuf,
		_id: RunId,
	) -> RunFuture<'_, Result<crate::ExecutionMetadata, CoreError>> {
		Box::pin(async {
			Err(CoreError::conflict(
				"execution.unavailable",
				"execution metadata is unavailable",
			))
		})
	}
	/// Performs the adoption checks without connecting a Craft or releasing source.
	fn validate_recovery(
		&self,
		_home: PathBuf,
		_id: RunId,
		_plan: LaunchPlan,
		_cursor: RunRecoveryCursor,
	) -> RunFuture<'_, Result<(), RunRecoveryError>> {
		Box::pin(async { Err(RunRecoveryError::Unsafe) })
	}
	/// Delivers one escalation step to the live native process group of a
	/// managed Run, leaving the helper and its retained source alive so the
	/// native exit still reaches Core through the ordinary source path.
	/// Implementations validate the helper identity before signalling and
	/// never choose the target from client input (ADR-0083).
	fn signal(
		&self,
		_home: PathBuf,
		_run_id: RunId,
		_signal: crate::ExecutionSignal,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async {
			Err(CoreError::conflict(
				"execution.unavailable",
				"execution control is unavailable",
			))
		})
	}
	/// Terminates only the instance selected by an authenticated interactive user.
	fn terminate(
		&self,
		_home: PathBuf,
		_request: crate::ExecutionResolution,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async {
			Err(CoreError::conflict(
				"execution.unavailable",
				"execution control is unavailable",
			))
		})
	}
}
/// Outcome at the external launch boundary.
#[derive(Debug)]
pub enum RunStartError {
	/// No native process could have started.
	NotStarted,
	/// The request may have reached a live execution; never retry it blindly.
	Unknown,
}
/// One authenticated, pinned Run connection. No protocol DTO crosses this port.
pub trait RunConnection: Send + Sync {
	/// Delivers one durably claimed turn; errors leave its outcome uncertain.
	fn submit_turn(
		&self,
		turn_id: uuid::Uuid,
		prompt: String,
	) -> RunFuture<'_, Result<(), CoreError>>;
	/// Receives a domain observation, validating transport identities first.
	fn receive(&self) -> RunFuture<'_, Result<Observation, CoreError>>;
	/// Whether the pinned Craft negotiated native turn cancellation. A
	/// connection that answers `false` is only ever stopped by signal
	/// escalation (ADR-0083).
	fn supports_native_cancellation(&self) -> bool;
	/// Asks the Harness to cancel one delivered turn, leaving the Run able
	/// to accept the next one. Implementations may assume Core admitted the
	/// request durably first.
	fn interrupt(
		&self,
		turn_id: uuid::Uuid,
	) -> RunFuture<'_, Result<(), CoreError>>;
	/// Releases native source only after Core committed its meaning.
	fn acknowledge(&self, offset: u64) -> RunFuture<'_, Result<(), CoreError>>;
	/// Closes the execution connection after its terminal source is committed.
	fn finish(&self) -> RunFuture<'_, Result<(), CoreError>>;
}

/// Durable source and process evidence supplied to recovery by Core.
#[derive(Debug, Clone)]
pub struct RunRecoveryCursor {
	/// Last committed source boundary.
	pub offset: u64,
	/// Parser state at that boundary.
	pub checkpoint: String,
	/// Previously recorded helper process, if startup reached that point.
	pub helper_pid: Option<u32>,
}
/// A recovery refusal never authorizes another native launch.
#[derive(Debug, PartialEq, Eq)]
pub enum RunRecoveryError {
	/// The previous execution process is proven gone.
	Gone,
	/// Ownership, identity, state, roots, or offsets could not be validated.
	Unsafe,
	/// A validated execution's Craft cannot currently reconnect.
	Unavailable,
}
