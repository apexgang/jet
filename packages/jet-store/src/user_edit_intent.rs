//! Restart-safe write-ahead intents for direct user edits (ADR-0064).

use uuid::Uuid;

use crate::{
	ActorRecord, NewUserEditIntent, ReadTransaction, StoreError,
	UserEditIntentRecord, WriteTransaction,
};

impl ReadTransaction {
	/// Finds one pending edit in its Actor-scoped Command identity.
	pub async fn user_edit_intent(
		&mut self,
		actor: ActorRecord,
		command_id: Uuid,
	) -> Result<Option<UserEditIntentRecord>, StoreError> {
		let (actor_kind, actor_id) = actor.columns();
		let actor_id = actor_id.to_string();
		let command_id = command_id.to_string();
		let row = sqlx::query_as!(
			RawUserEditIntent,
			"SELECT actor_kind, actor_id, command_id, request_digest, \
			 recorded_at_unix_ms, plan FROM user_edit_intents \
			 WHERE actor_kind = ?1 AND actor_id = ?2 AND command_id = ?3",
			actor_kind,
			actor_id,
			command_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read).transpose()
	}

	/// Lists every pending edit for startup reconciliation.
	pub async fn user_edit_intents(
		&mut self,
	) -> Result<Vec<UserEditIntentRecord>, StoreError> {
		sqlx::query_as!(
			RawUserEditIntent,
			"SELECT actor_kind, actor_id, command_id, request_digest, \
			 recorded_at_unix_ms, plan FROM user_edit_intents ORDER BY rowid"
		)
		.fetch_all(self.connection())
		.await?
		.into_iter()
		.map(read)
		.collect()
	}
}

impl WriteTransaction {
	/// Inserts an edit intent unless that Actor and Command already have one.
	pub async fn insert_user_edit_intent(
		&mut self,
		intent: &NewUserEditIntent,
	) -> Result<(), StoreError> {
		let (actor_kind, actor_id) = intent.actor.columns();
		let actor_id = actor_id.to_string();
		let command_id = intent.command_id.to_string();
		let request_digest = intent.request_digest.as_slice();
		sqlx::query!(
			"INSERT INTO user_edit_intents \
			 (actor_kind, actor_id, command_id, request_digest, \
			  recorded_at_unix_ms, plan) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
			 ON CONFLICT(actor_kind, actor_id, command_id) DO NOTHING",
			actor_kind,
			actor_id,
			command_id,
			request_digest,
			intent.recorded_at_unix_ms,
			intent.plan
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Removes the intent in the same transaction that records its outcome.
	pub async fn delete_user_edit_intent(
		&mut self,
		actor: ActorRecord,
		command_id: Uuid,
	) -> Result<(), StoreError> {
		let (actor_kind, actor_id) = actor.columns();
		let actor_id = actor_id.to_string();
		let command_id = command_id.to_string();
		sqlx::query!(
			"DELETE FROM user_edit_intents \
			 WHERE actor_kind = ?1 AND actor_id = ?2 AND command_id = ?3",
			actor_kind,
			actor_id,
			command_id
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

struct RawUserEditIntent {
	actor_kind: String,
	actor_id: String,
	command_id: String,
	request_digest: Vec<u8>,
	recorded_at_unix_ms: i64,
	plan: String,
}

fn read(row: RawUserEditIntent) -> Result<UserEditIntentRecord, StoreError> {
	let digest_length = row.request_digest.len();
	Ok(UserEditIntentRecord {
		actor: ActorRecord::parse(&row.actor_kind, &row.actor_id)?,
		command_id: row.command_id.parse().map_err(|_| {
			StoreError::Integrity("invalid user edit command identity".into())
		})?,
		request_digest: row.request_digest.try_into().map_err(|_| {
			StoreError::Integrity(format!(
				"user edit digest has {digest_length} bytes"
			))
		})?,
		recorded_at_unix_ms: row.recorded_at_unix_ms,
		plan: row.plan,
	})
}
