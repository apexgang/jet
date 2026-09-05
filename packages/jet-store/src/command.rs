//! Durable Actor-scoped Command receipts (ADR-0093).

use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	ActorRecord, CommandReceiptRecord, NewCommandReceipt, column_error,
	parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

impl ReadTransaction {
	/// Finds the receipt for `command_id` in `actor`'s identity scope.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the receipt cannot be read.
	pub async fn command_receipt(
		&mut self,
		actor: ActorRecord,
		command_id: Uuid,
	) -> Result<Option<CommandReceiptRecord>, StoreError> {
		let (actor_kind, actor_id) = actor.columns();
		let actor_id = actor_id.to_string();
		let command_id = command_id.to_string();
		let row = sqlx::query!(
			"SELECT actor_kind, actor_id, command_id, request_digest,
				recorded_at_unix_ms, outcome_version, outcome
			 FROM command_receipts
			 WHERE actor_kind = ?1 AND actor_id = ?2 AND command_id = ?3",
			actor_kind,
			actor_id,
			command_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(|row| {
			Ok(CommandReceiptRecord {
				actor: ActorRecord::parse(&row.actor_kind, &row.actor_id)?,
				command_id: parse_uuid("command_id", &row.command_id)?,
				request_digest: row
					.request_digest
					.map(parse_digest)
					.transpose()?,
				recorded_at_unix_ms: row.recorded_at_unix_ms,
				outcome_version: row
					.outcome_version
					.map(parse_outcome_version)
					.transpose()?,
				outcome: row.outcome,
			})
		})
		.transpose()
	}
}

impl WriteTransaction {
	/// Records an accepted Command's identity and authoritative outcome.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the receipt cannot be written.
	pub async fn insert_command_receipt(
		&mut self,
		receipt: &NewCommandReceipt,
	) -> Result<(), StoreError> {
		let (actor_kind, actor_id) = receipt.actor.columns();
		let actor_id = actor_id.to_string();
		let command_id = receipt.command_id.to_string();
		let request_digest = receipt.request_digest.as_slice();
		let outcome_version = i64::from(receipt.outcome_version);
		sqlx::query!(
			"INSERT INTO command_receipts (
				actor_kind, actor_id, command_id, request_digest,
				recorded_at_unix_ms, outcome_version, outcome
			 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
			actor_kind,
			actor_id,
			command_id,
			request_digest,
			receipt.recorded_at_unix_ms,
			outcome_version,
			receipt.outcome
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Discards digests and outcomes whose retry window ended while keeping
	/// their Actor-scoped identities as permanent expiry tombstones.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the receipts cannot be pruned.
	pub async fn prune_command_receipts_before(
		&mut self,
		cutoff_unix_ms: i64,
	) -> Result<(), StoreError> {
		sqlx::query!(
			"UPDATE command_receipts
			 SET request_digest = NULL, outcome_version = NULL, outcome = NULL
			 WHERE recorded_at_unix_ms < ?1 AND request_digest IS NOT NULL",
			cutoff_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

fn parse_digest(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
	let length = bytes.len();
	bytes.try_into().map_err(|_| {
		column_error(
			"request_digest",
			format!("command digest has {length} bytes"),
		)
	})
}

fn parse_outcome_version(version: i64) -> Result<u32, StoreError> {
	u32::try_from(version).map_err(|_| {
		column_error(
			"outcome_version",
			format!("outcome version {version} is out of range"),
		)
	})
}
