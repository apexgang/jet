//! Commands: authenticated, durable mutations. Each one commits its
//! current-state change and its journal Event in one transaction and is
//! acknowledged only after that commit (ADR-0020, ADR-0071).

use jet_store::{
	CommandReceiptRecord, EffectKindRecord, EffectSafetyRecord,
	NewCommandReceipt, NewConversation, NewEffect, NewRun, RetentionPolicy,
	RunLifecycle, WriteTransaction,
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
	command_id: CommandId,
	/// Requested mutation.
	command: Command,
	request_digest: [u8; 32],
}

impl CommandEnvelope {
	/// Builds an envelope and binds its typed Command to a digest of the exact
	/// encoded body. The encoded bytes include unknown compatible fields and
	/// representation differences, so only a byte-equivalent retry reuses the
	/// result.
	///
	/// # Errors
	///
	/// Returns an internal error if the typed Command cannot be encoded for
	/// the binding digest.
	pub fn new(
		command_id: CommandId,
		command: Command,
		request_bytes: &[u8],
	) -> Result<Self, CoreError> {
		let command_bytes = serde_json::to_vec(&command).map_err(|error| {
			CoreError::internal("command.encode_failed", error.to_string())
		})?;
		let mut digest = Sha256::new();
		digest.update(
			u64::try_from(request_bytes.len())
				.unwrap_or(u64::MAX)
				.to_be_bytes(),
		);
		digest.update(request_bytes);
		digest.update(command_bytes);
		Ok(Self {
			command_id,
			command,
			request_digest: digest.finalize().into(),
		})
	}
}

/// A state-changing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Command {
	/// Create a Conversation with no Runs.
	CreateConversation {
		/// Whether Jet keeps the Conversation after its final Run.
		retention: RetentionPolicy,
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
		let recorded_at_unix_ms = self.now_unix_ms();
		self.store.write(|tx| {
			tx.prune_command_receipts_before(
				recorded_at_unix_ms.saturating_sub(COMMAND_RETENTION_MS),
			)?;
			if let Some(receipt) =
				tx.command_receipt(actor_record, command_id.0)?
			{
				return replay(receipt, request_digest, recorded_at_unix_ms);
			}

			let result = execute_new(
				tx,
				actor,
				command_id,
				command,
				recorded_at_unix_ms,
			);
			if let Err(error) = &result
				&& !error.is_authoritative_result()
			{
				return Err(error.clone());
			}
			// An authoritative error is raised before the Command writes any
			// state, so committing its receipt commits nothing else. A Command
			// that must fail authoritatively after writing wraps its writes in
			// a savepoint first.
			tx.insert_command_receipt(&NewCommandReceipt {
				actor: actor_record,
				command_id: command_id.0,
				request_digest,
				recorded_at_unix_ms,
				outcome_version: OUTCOME_VERSION,
				outcome: encode_result(&result)?,
			})?;
			Ok(result)
		})?
	}
}

fn replay(
	receipt: CommandReceiptRecord,
	request_digest: [u8; 32],
	now_unix_ms: i64,
) -> Result<Result<CommandOutcome, CoreError>, CoreError> {
	if now_unix_ms.saturating_sub(receipt.recorded_at_unix_ms)
		> COMMAND_RETENTION_MS
	{
		return Ok(Err(CoreError::invalid_input(
			"command.identity_expired",
			"the Command identity is older than thirty days",
		)));
	}
	let Some(original_digest) = receipt.request_digest else {
		return Err(invalid_receipt("digest"));
	};
	if original_digest != request_digest {
		return Err(CoreError::conflict(
			"command.identity_reused",
			"the Command identity was already used for different content",
		));
	}
	let Some(outcome_version) = receipt.outcome_version else {
		return Err(invalid_receipt("outcome version"));
	};
	if outcome_version != OUTCOME_VERSION {
		return Ok(Err(CoreError::incompatible(
			"command.outcome_incompatible",
			"the Command outcome was recorded by an incompatible core; submit the Command again under a new identity",
		)));
	}
	let Some(outcome) = receipt.outcome else {
		return Err(invalid_receipt("outcome"));
	};
	serde_json::from_str(&outcome).map_err(|error| {
		CoreError::internal("command.outcome_invalid", error.to_string())
	})
}

fn invalid_receipt(missing: &str) -> CoreError {
	CoreError::internal(
		"command.receipt_invalid",
		format!("an unexpired Command receipt has no {missing}"),
	)
}

fn execute_new(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	command_id: CommandId,
	command: Command,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	match command {
		Command::CreateConversation { retention } => {
			create_conversation(tx, actor, retention, now_unix_ms)
		}
		Command::CreateRun { conversation_id } => {
			create_run(tx, actor, conversation_id, now_unix_ms)
		}
		Command::TransitionRun {
			run_id,
			expected_revision,
			lifecycle,
		} => transition_run(
			tx,
			actor,
			command_id,
			run_id,
			expected_revision,
			lifecycle,
			now_unix_ms,
		),
	}
}

fn encode_result(
	result: &Result<CommandOutcome, CoreError>,
) -> Result<String, CoreError> {
	serde_json::to_string(result).map_err(|error| {
		CoreError::internal("command.outcome_encode_failed", error.to_string())
	})
}

fn create_conversation(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	retention: RetentionPolicy,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
			created_at_unix_ms: now_unix_ms,
		})?
		.into();
	let event = EventKind::ConversationCreated { retention };
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(conversation.conversation_id),
		now_unix_ms,
	)?)?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

fn create_run(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	conversation_id: ConversationId,
	now_unix_ms: i64,
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
			"the Conversation already has a Run that has not ended",
		));
	}
	let run: Run = tx
		.insert_run(NewRun {
			run_id: Uuid::now_v7(),
			conversation_id: conversation_id.0,
			created_at_unix_ms: now_unix_ms,
		})?
		.into();
	tx.append_event(EventKind::RunCreated {}.to_record(
		actor,
		EventSubject::Run {
			conversation_id,
			run_id: run.run_id,
		},
		now_unix_ms,
	)?)?;
	Ok(CommandOutcome::RunCreated(run))
}

fn transition_run(
	tx: &WriteTransaction<'_>,
	actor: &Actor,
	command_id: CommandId,
	run_id: RunId,
	expected_revision: Revision,
	lifecycle: RunLifecycle,
	now_unix_ms: i64,
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
	let run: Run = tx
		.update_run_lifecycle(run_id.0, lifecycle, now_unix_ms)?
		.into();
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
		now_unix_ms,
	)?)?;
	if lifecycle == RunLifecycle::Starting {
		let effect_id = Uuid::now_v7();
		// ASVS 2.3.3: the Effect is inserted in the same authoritative
		// transaction as the Run state and Event before acknowledgement.
		tx.insert_effect(&NewEffect {
			effect_id,
			command_id: command_id.0,
			run_id: Some(run_id.0),
			kind: EffectKindRecord::StartRun,
			safety: EffectSafetyRecord::Idempotent {
				external_key: effect_id,
				max_attempts: 3,
			},
		})?;
	}
	Ok(CommandOutcome::RunTransitioned(run))
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
