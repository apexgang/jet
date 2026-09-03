//! Commands: authenticated, durable mutations. Each one commits its
//! current-state change and its journal Event in one transaction and is
//! acknowledged only after that commit (ADR-0020, ADR-0071).

use jet_store::{
	NewCommandReceipt, NewConversation, NewRun, Retention, RunLifecycle,
	WriteTransaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::conversation::{Conversation, ConversationId, Revision, Run, RunId};
use crate::error::{ConflictState, CoreError, RevisionConflict};
use crate::event::{EventKind, EventSubject};
use crate::{Actor, Core, lifecycle};

const OUTCOME_VERSION: u32 = 1;
const COMMAND_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Actor-scoped identity of a Command, retained for retry safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub Uuid);

/// One Command with the identity and exact request bytes used for retry
/// safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandEnvelope {
	/// Actor-scoped identity of the Command.
	pub command_id: CommandId,
	/// Requested mutation.
	pub command: Command,
	request_digest: [u8; 32],
}

impl CommandEnvelope {
	/// Builds an envelope and digests the exact wire bytes of its Command
	/// body. Bytes include unknown compatible fields and representation
	/// differences, so only a byte-equivalent retry reuses the result.
	#[must_use]
	pub fn new(
		command_id: CommandId,
		command: Command,
		request_bytes: &[u8],
	) -> Self {
		Self {
			command_id,
			command,
			request_digest: Sha256::digest(request_bytes).into(),
		}
	}
}

/// A state-changing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
		/// Revision observed when the Command was prepared.
		expected_revision: Revision,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
}

/// The durable result of a [`Command`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
		envelope: CommandEnvelope,
	) -> Result<CommandOutcome, CoreError> {
		actor.authorize()?;
		let CommandEnvelope {
			command_id,
			command,
			request_digest,
		} = envelope;
		let actor_record = actor.record();
		let recorded_at_unix_ms = crate::unix_ms(self.clock.now());
		self.store.write(|tx| {
			if let Some(receipt) =
				tx.command_receipt(actor_record, command_id.0)?
			{
				if recorded_at_unix_ms
					.saturating_sub(receipt.recorded_at_unix_ms)
					> COMMAND_RETENTION_MS
				{
					return Err(CoreError::invalid_input(
						"command.identity_expired",
						"the Command identity is older than thirty days",
					));
				}
				if receipt.request_digest != request_digest {
					return Err(CoreError::conflict(
						"command.identity_reused",
						"the Command identity was already used for different content"
							.into(),
					));
				}
				if receipt.outcome_version != OUTCOME_VERSION {
					return Err(CoreError::internal(
						"command.outcome_incompatible",
						format!(
							"unsupported Command outcome version {}",
							receipt.outcome_version
						),
					));
				}
				let original = serde_json::from_str(&receipt.outcome).map_err(
					|error| {
						CoreError::internal(
							"command.outcome_invalid",
							error.to_string(),
						)
					},
				)?;
				return Ok(original);
			}

			let result = match command {
				Command::CreateConversation { retention } => {
					create_conversation(tx, actor, retention)
				}
				Command::CreateRun { conversation_id } => {
					create_run(tx, actor, conversation_id)
				}
				Command::TransitionRun {
					run_id,
					expected_revision,
					lifecycle,
				} => transition_run(
					tx,
					actor,
					run_id,
					expected_revision,
					lifecycle,
				),
			};
			if let Err(error) = &result
				&& !error.is_authoritative_result()
			{
				return Err(error.clone());
			}
			let encoded = serde_json::to_string(&result).map_err(|error| {
				CoreError::internal(
					"command.outcome_encode_failed",
					error.to_string(),
				)
			})?;
			tx.insert_command_receipt(&NewCommandReceipt {
				actor: actor_record,
				command_id: command_id.0,
				request_digest,
				recorded_at_unix_ms,
				outcome_version: OUTCOME_VERSION,
				outcome: encoded,
			})?;
			Ok(result)
		})?
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
		EventSubject::Conversation(conversation.conversation_id),
	)?)?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

fn create_run(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	conversation_id: ConversationId,
) -> Result<CommandOutcome, CoreError> {
	if tx.conversation(conversation_id.0)?.is_none() {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	}
	let busy = tx
		.runs(conversation_id.0)?
		.iter()
		.any(|run| !run.lifecycle.is_terminal());
	if busy {
		return Err(CoreError::conflict(
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
		EventSubject::Run {
			conversation_id,
			run_id: run.run_id,
		},
	)?)?;
	Ok(CommandOutcome::RunCreated(run))
}

fn transition_run(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	run_id: RunId,
	expected_revision: Revision,
	lifecycle: RunLifecycle,
) -> Result<CommandOutcome, CoreError> {
	let Some(current) = tx.run(run_id.0)? else {
		return Err(CoreError::not_found(
			"run.not_found",
			"the Run does not exist",
		));
	};
	if current.revision != expected_revision.0 {
		let current: Run = current.into();
		return Err(CoreError::revision_conflict(
			"run.revision_conflict",
			"the Run changed since the Command was prepared",
			RevisionConflict {
				current_revision: current.revision,
				safe_state: ConflictState::Run(current),
			},
		));
	}
	if !lifecycle::may_transition(current.lifecycle, lifecycle) {
		return Err(CoreError::conflict(
			"run.invalid_transition",
			format!(
				"a {} Run cannot move to {}",
				current.lifecycle.as_str(),
				lifecycle.as_str()
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
		EventSubject::Run {
			conversation_id: run.conversation_id,
			run_id,
		},
	)?)?;
	Ok(CommandOutcome::RunTransitioned(run))
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
