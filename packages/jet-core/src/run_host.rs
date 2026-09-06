//! Execution port: jetd translates Craft/helper traffic into these domain observations.
use crate::{
	CoreError, RunId, run_command::LaunchPlan, run_craft::PinnedCraft,
	run_state::Observation,
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
	/// Starts the accepted execution, distinguishing rejection from uncertainty.
	fn start(
		&self,
		home: PathBuf,
		run_id: RunId,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>>;
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
pub trait RunConnection: Send {
	/// Receives a domain observation, validating transport identities first.
	fn receive(&mut self) -> RunFuture<'_, Result<Observation, CoreError>>;
	/// Releases native source only after Core committed its meaning.
	fn acknowledge(
		&mut self,
		offset: u64,
	) -> RunFuture<'_, Result<(), CoreError>>;
	/// Closes the execution connection after its terminal source is committed.
	fn finish(&mut self) -> RunFuture<'_, Result<(), CoreError>>;
}
