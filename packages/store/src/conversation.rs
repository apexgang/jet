//! Current state of Conversations, independent of any Run (ADR-0001).

use rusqlite::{OptionalExtension, Row};
use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	ConversationRecord, NewConversation, RetentionPolicy, column_error,
	parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

const COLUMNS: &str = "conversation_id, retention, created_at_unix_ms";

impl ReadTransaction<'_> {
	/// The Conversation identified by `conversation_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub fn conversation(
		&self,
		conversation_id: Uuid,
	) -> Result<Option<ConversationRecord>, StoreError> {
		Ok(self
			.transaction
			.query_row(
				&format!(
					"SELECT {COLUMNS} FROM conversations
					 WHERE conversation_id = ?1"
				),
				[conversation_id.to_string()],
				read_row,
			)
			.optional()?)
	}

	/// Every recorded Conversation in creation order.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub fn conversations(&self) -> Result<Vec<ConversationRecord>, StoreError> {
		let mut statement = self.transaction.prepare(&format!(
			"SELECT {COLUMNS} FROM conversations ORDER BY rowid"
		))?;
		let rows = statement.query_map([], read_row)?;
		Ok(rows.collect::<Result<_, _>>()?)
	}
}

impl WriteTransaction<'_> {
	/// Records a new Conversation with no Runs.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the identity is already taken.
	pub fn insert_conversation(
		&self,
		conversation: NewConversation,
	) -> Result<ConversationRecord, StoreError> {
		let record = ConversationRecord {
			conversation_id: conversation.conversation_id,
			retention: conversation.retention,
			created_at_unix_ms: conversation.created_at_unix_ms,
		};
		self.transaction.execute(
			&format!(
				"INSERT INTO conversations ({COLUMNS}) VALUES (?1, ?2, ?3)"
			),
			(
				record.conversation_id.to_string(),
				record.retention.as_str(),
				record.created_at_unix_ms,
			),
		)?;
		Ok(record)
	}
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<ConversationRecord> {
	let conversation_id: String = row.get(0)?;
	let retention: String = row.get(1)?;
	Ok(ConversationRecord {
		conversation_id: parse_uuid(0, &conversation_id)?,
		retention: parse_retention(&retention)?,
		created_at_unix_ms: row.get(2)?,
	})
}

fn parse_retention(text: &str) -> rusqlite::Result<RetentionPolicy> {
	RetentionPolicy::parse(text).ok_or_else(|| {
		column_error(1, format!("unknown retention value {text:?}"))
	})
}
