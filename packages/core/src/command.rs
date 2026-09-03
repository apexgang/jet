//! Commands: authenticated, durable mutations. Each one commits its
//! current-state change and its journal Event in one transaction and is
//! acknowledged only after that commit (ADR-0020, ADR-0071).

use jet_store::{
	NewConversation, NewRun, Retention, RunLifecycle, WriteTransaction,
};
use uuid::Uuid;

use crate::conversation::{Conversation, ConversationId, Run, RunId};
use crate::error::{CoreError, ErrorCategory};
use crate::event::EventKind;
use crate::{Actor, Core, lifecycle};

/// A state-changing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
	/// Create a Conversation with no Runs.
	CreateConversation {
		/// Whether Jet keeps the Conversation after its final Run.
		retention: Retention,
	},
	/// Record a new Run of a Conversation that has no live Run.
	CreateRun {
		/// The Conversation to execute.
		conversation_id: ConversationId,
	},
	/// Move a Run forward through its lifecycle.
	TransitionRun {
		/// The Run to move.
		run_id: RunId,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
}

/// The durable result of a [`Command`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
	/// The Conversation as created.
	ConversationCreated(Conversation),
	/// The Run as created.
	RunCreated(Run),
	/// The Run after its transition.
	RunTransitioned(Run),
}

impl Core {
	/// Executes `command` on behalf of `actor`. The outcome is durable when
	/// this returns `Ok`.
	///
	/// # Errors
	///
	/// Returns a `not_found` [`CoreError`] for unknown identities, a
	/// `conflict` one when the Command violates a lifecycle invariant, and
	/// a store category when the transaction cannot commit.
	pub fn execute(
		&self,
		actor: &Actor,
		command: Command,
	) -> Result<CommandOutcome, CoreError> {
		// Every authenticated local Actor may drive Conversations.
		match actor {
			Actor::InteractiveClient { .. } => {}
		}
		self.store.write(|tx| match command {
			Command::CreateConversation { retention } => {
				create_conversation(tx, actor, retention)
			}
			Command::CreateRun { conversation_id } => {
				create_run(tx, actor, conversation_id)
			}
			Command::TransitionRun { run_id, lifecycle } => {
				transition_run(tx, actor, run_id, lifecycle)
			}
		})
	}
}

fn create_conversation(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	retention: Retention,
) -> Result<CommandOutcome, CoreError> {
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
		})?
		.into();
	let event = EventKind::ConversationCreated { retention };
	tx.append_event(event.to_record(
		actor,
		conversation.conversation_id,
		None,
	)?)?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

fn create_run(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	conversation_id: ConversationId,
) -> Result<CommandOutcome, CoreError> {
	if tx.conversation(conversation_id.0)?.is_none() {
		return Err(not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	}
	let busy = tx
		.runs(conversation_id.0)?
		.iter()
		.any(|run| !run.lifecycle.is_terminal());
	if busy {
		return Err(conflict(
			"run.conversation_busy",
			"the Conversation already has a Run that has not ended".into(),
		));
	}
	let run: Run = tx
		.insert_run(NewRun {
			run_id: Uuid::now_v7(),
			conversation_id: conversation_id.0,
		})?
		.into();
	tx.append_event(EventKind::RunCreated {}.to_record(
		actor,
		conversation_id,
		Some(run.run_id),
	)?)?;
	Ok(CommandOutcome::RunCreated(run))
}

fn transition_run(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	run_id: RunId,
	lifecycle: RunLifecycle,
) -> Result<CommandOutcome, CoreError> {
	let Some(current) = tx.run(run_id.0)? else {
		return Err(not_found("run.not_found", "the Run does not exist"));
	};
	if !lifecycle::may_transition(current.lifecycle, lifecycle) {
		return Err(conflict(
			"run.invalid_transition",
			format!(
				"a {} Run cannot move to {}",
				lifecycle::name(current.lifecycle),
				lifecycle::name(lifecycle)
			),
		));
	}
	let run: Run = tx.update_run_lifecycle(run_id.0, lifecycle)?.into();
	let event = EventKind::RunLifecycleChanged {
		from: current.lifecycle,
		to: lifecycle,
	};
	tx.append_event(event.to_record(
		actor,
		run.conversation_id,
		Some(run_id),
	)?)?;
	Ok(CommandOutcome::RunTransitioned(run))
}

pub(crate) fn not_found(code: &'static str, message: &str) -> CoreError {
	CoreError {
		category: ErrorCategory::NotFound,
		code,
		retryable: false,
		message: message.into(),
		detail: None,
	}
}

fn conflict(code: &'static str, message: String) -> CoreError {
	CoreError {
		category: ErrorCategory::Conflict,
		code,
		retryable: false,
		message,
		detail: None,
	}
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
