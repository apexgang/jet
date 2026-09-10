//! Reconciles durable User-edit intents after interruption.

use super::{PreparedUserEdit, apply};
use crate::{Actor, CommandId, CommandOutcome, Core, CoreError};
use jet_store::{NewCommandReceipt, UserEditIntentRecord, WriteTransaction};

pub(super) async fn abandon(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	intent_recorded: bool,
	error: CoreError,
) -> Result<CommandOutcome, CoreError> {
	if intent_recorded && error.is_authoritative_result() {
		tx.delete_user_edit_intent(actor.record(), command_id.0)
			.await?;
	}
	Err(error)
}

pub(super) fn resume(
	intent: UserEditIntentRecord,
	request_digest: [u8; 32],
) -> Result<PreparedUserEdit, CoreError> {
	if intent.request_digest != request_digest {
		return Err(CoreError::conflict(
			"command.identity_reused",
			"the Command identity was already used for different content",
		));
	}
	let mut prepared: PreparedUserEdit = serde_json::from_str(&intent.plan)
		.map_err(|error| {
			CoreError::internal("user_edit.intent_invalid", error.to_string())
		})?;
	prepared.resumed = true;
	Ok(prepared)
}

impl Core {
	/// Finishes direct edits whose write-ahead intent survived an interruption.
	///
	/// # Errors
	///
	/// Returns a store or filesystem error when reconciliation cannot yet make
	/// progress. Authoritative conflicts are retained as Command receipts.
	pub async fn perform_user_edits(&self) -> Result<(), CoreError> {
		let intents = self
			.store
			.read(async |tx| tx.user_edit_intents().await)
			.await?;
		for intent in intents {
			let actor = Actor::from_record(intent.actor);
			let command_id = CommandId(intent.command_id);
			let prepared = resume(intent.clone(), intent.request_digest)?;
			self.store
				.write(async |tx| {
					if tx
						.command_receipt(intent.actor, intent.command_id)
						.await?
						.is_some()
					{
						tx.delete_user_edit_intent(
							intent.actor,
							intent.command_id,
						)
						.await?;
						return Ok(());
					}
					let result = apply(
						self,
						tx,
						&actor,
						command_id,
						prepared,
						intent.recorded_at_unix_ms,
					)
					.await;
					if let Err(error) = &result {
						if !error.is_authoritative_result() {
							return Err(error.clone());
						}
						tx.delete_user_edit_intent(
							intent.actor,
							intent.command_id,
						)
						.await?;
					}
					tx.insert_command_receipt(&NewCommandReceipt {
						actor: intent.actor,
						command_id: intent.command_id,
						request_digest: intent.request_digest,
						recorded_at_unix_ms: intent.recorded_at_unix_ms,
						outcome_version:
							crate::command::receipt::OUTCOME_VERSION,
						outcome: crate::command::receipt::encode_result(
							&result,
						)?,
					})
					.await?;
					Ok::<_, CoreError>(())
				})
				.await?;
		}
		Ok(())
	}
}
