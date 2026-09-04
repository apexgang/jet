//! Current state of Conversations, independent of any Run (ADR-0001).

use rusqlite::{OptionalExtension, Row};
use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	ConversationPageKey, ConversationPageStart, ConversationRecord,
	NewConversation, RetentionPolicy, column_error, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

const COLUMNS: &str = "conversation_id, retention, created_at_unix_ms";

/// Most Conversations returned by one keyset page.
pub const CONVERSATION_PAGE_LIMIT: usize = 256;

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

	/// One bounded keyset page of Conversations in creation order.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub fn conversation_page(
		&self,
		start: ConversationPageStart,
	) -> Result<
		(Vec<ConversationRecord>, Option<ConversationPageKey>),
		StoreError,
	> {
		let after = match start {
			ConversationPageStart::First => 0,
			ConversationPageStart::After(key) => key.0,
		};
		// ASVS 1.2.4 and 2.2.2: the key is parameterized and the trusted
		// store fixes the allocation bound rather than accepting one from a
		// protocol caller.
		let mut statement = self.transaction.prepare(&format!(
			"SELECT rowid, {COLUMNS} FROM conversations
			 WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"
		))?;
		let rows = statement.query_map(
			(
				after,
				i64::try_from(CONVERSATION_PAGE_LIMIT + 1).unwrap_or(i64::MAX),
			),
			|row| Ok((ConversationPageKey(row.get(0)?), read_row_at(row, 1)?)),
		)?;
		let mut rows = rows.collect::<Result<Vec<_>, _>>()?;
		let next = (rows.len() > CONVERSATION_PAGE_LIMIT)
			.then(|| rows[CONVERSATION_PAGE_LIMIT - 1].0);
		rows.truncate(CONVERSATION_PAGE_LIMIT);
		Ok((rows.into_iter().map(|(_, record)| record).collect(), next))
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
	read_row_at(row, 0)
}

fn read_row_at(
	row: &Row<'_>,
	offset: usize,
) -> rusqlite::Result<ConversationRecord> {
	let conversation_id: String = row.get(offset)?;
	let retention: String = row.get(offset + 1)?;
	Ok(ConversationRecord {
		conversation_id: parse_uuid(offset, &conversation_id)?,
		retention: parse_retention(&retention)?,
		created_at_unix_ms: row.get(offset + 2)?,
	})
}

fn parse_retention(text: &str) -> rusqlite::Result<RetentionPolicy> {
	RetentionPolicy::parse(text).ok_or_else(|| {
		column_error(1, format!("unknown retention value {text:?}"))
	})
}
