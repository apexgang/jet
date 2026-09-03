//! Durable Actor-scoped Command receipts (ADR-0093).

use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	ActorRecord, CommandReceiptRecord, NewCommandReceipt, column_error,
	parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

impl ReadTransaction<'_> {
	/// Finds the receipt for `command_id` in `actor`'s identity scope.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the receipt cannot be read.
	pub fn command_receipt(
		&self,
		actor: ActorRecord,
		command_id: Uuid,
	) -> Result<Option<CommandReceiptRecord>, StoreError> {
		let (actor_kind, actor_id) = actor.columns();
		Ok(self
			.transaction
			.query_row(
				"SELECT actor_kind, actor_id, command_id, request_digest,
					recorded_at_unix_ms, outcome_version, outcome
				 FROM command_receipts
				 WHERE actor_kind = ?1 AND actor_id = ?2 AND command_id = ?3",
				(actor_kind, actor_id.to_string(), command_id.to_string()),
				|row| {
					let actor_kind: String = row.get(0)?;
					let actor_id: String = row.get(1)?;
					let command_id: String = row.get(2)?;
					let digest: Vec<u8> = row.get(3)?;
					let request_digest: [u8; 32] =
						digest.try_into().map_err(|bytes: Vec<u8>| {
							column_error(
								3,
								format!(
									"command digest has {} bytes",
									bytes.len()
								),
							)
						})?;
					Ok(CommandReceiptRecord {
						actor: ActorRecord::parse(&actor_kind, &actor_id, 1)?,
						command_id: parse_uuid(2, &command_id)?,
						request_digest,
						recorded_at_unix_ms: row.get(4)?,
						outcome_version: row.get(5)?,
						outcome: row.get(6)?,
					})
				},
			)
			.optional()?)
	}
}

impl WriteTransaction<'_> {
	/// Records an accepted Command's identity and authoritative outcome.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the receipt cannot be written.
	pub fn insert_command_receipt(
		&self,
		receipt: &NewCommandReceipt,
	) -> Result<(), StoreError> {
		let (actor_kind, actor_id) = receipt.actor.columns();
		self.transaction.execute(
			"INSERT INTO command_receipts (
				actor_kind, actor_id, command_id, request_digest,
				recorded_at_unix_ms, outcome_version, outcome
			 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
			(
				actor_kind,
				actor_id.to_string(),
				receipt.command_id.to_string(),
				&receipt.request_digest[..],
				receipt.recorded_at_unix_ms,
				receipt.outcome_version,
				&receipt.outcome,
			),
		)?;
		Ok(())
	}
}
