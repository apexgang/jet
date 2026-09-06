//! Imported conversations: Harness-native Conversation identities
//! discovered outside Jet and registered so a managed Run can continue
//! them (ADR-0010).
//!
//! A row keeps the identity as the Harness spells it, the directory the
//! Harness reported working in, and the Actor that registered it. Whether a
//! live process still holds the identity is observed, never stored. The
//! Conversation that continues an import, once one exists, points back at
//! the row from `conversations`, so a read joins it in rather than keeping
//! the link twice.

use uuid::Uuid;

use crate::StoreError;
use crate::records::{ActorRecord, parse_optional_uuid, parse_uuid};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// An Imported conversation to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewImportedConversation {
	/// Globally unique identity chosen by the caller.
	pub import_id: Uuid,
	/// The Harness whose native identity this is.
	pub harness: String,
	/// The identity as the Harness spells it.
	pub native_conversation: String,
	/// The directory the Harness reported working in, if it reported one.
	pub working_directory: Option<String>,
	/// The authenticated Actor that registered the identity.
	pub imported_by: ActorRecord,
	/// When the caller recorded the import.
	pub imported_at_unix_ms: i64,
}

/// One recorded Imported conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedConversationRecord {
	/// Globally unique identity.
	pub import_id: Uuid,
	/// The Harness whose native identity this is.
	pub harness: String,
	/// The identity as the Harness spells it.
	pub native_conversation: String,
	/// The directory the Harness reported working in, if it reported one.
	pub working_directory: Option<String>,
	/// The authenticated Actor that registered the identity.
	pub imported_by: ActorRecord,
	/// When the import was recorded.
	pub imported_at_unix_ms: i64,
	/// The Conversation that continues it, once one has been created.
	pub resumed_as: Option<Uuid>,
}

/// One `imported_conversations` row as SQLite stores it, with the
/// Conversation that continues it joined in.
struct Row {
	import_id: String,
	harness: String,
	native_conversation: String,
	working_directory: Option<String>,
	actor_kind: String,
	actor_id: String,
	imported_at_unix_ms: i64,
	resumed_as: Option<String>,
}

impl ReadTransaction {
	/// Every Imported conversation, in the order they were registered.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn imported_conversations(
		&mut self,
	) -> Result<Vec<ImportedConversationRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT i.import_id AS "import_id!", i.harness,
				i.native_conversation, i.working_directory, i.actor_kind,
				i.actor_id, i.imported_at_unix_ms,
				c.conversation_id AS "resumed_as?"
			 FROM imported_conversations AS i
			 LEFT JOIN conversations AS c ON c.import_id = i.import_id
			 ORDER BY i.rowid"#
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// The Imported conversation identified by `import_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn imported_conversation(
		&mut self,
		import_id: Uuid,
	) -> Result<Option<ImportedConversationRecord>, StoreError> {
		let import_id = import_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT i.import_id AS "import_id!", i.harness,
				i.native_conversation, i.working_directory, i.actor_kind,
				i.actor_id, i.imported_at_unix_ms,
				c.conversation_id AS "resumed_as?"
			 FROM imported_conversations AS i
			 LEFT JOIN conversations AS c ON c.import_id = i.import_id
			 WHERE i.import_id = ?1"#,
			import_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}

	/// The Imported conversation registered for one Harness-native
	/// identity, if any.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn imported_conversation_by_identity(
		&mut self,
		harness: &str,
		native_conversation: &str,
	) -> Result<Option<ImportedConversationRecord>, StoreError> {
		let row = sqlx::query_as!(
			Row,
			r#"SELECT i.import_id AS "import_id!", i.harness,
				i.native_conversation, i.working_directory, i.actor_kind,
				i.actor_id, i.imported_at_unix_ms,
				c.conversation_id AS "resumed_as?"
			 FROM imported_conversations AS i
			 LEFT JOIN conversations AS c ON c.import_id = i.import_id
			 WHERE i.harness = ?1 AND i.native_conversation = ?2"#,
			harness,
			native_conversation
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}
}

impl WriteTransaction {
	/// Records a new Imported conversation and returns it as stored, with
	/// no Conversation continuing it yet.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the same Harness-native identity is already registered.
	pub async fn insert_imported_conversation(
		&mut self,
		import: NewImportedConversation,
	) -> Result<ImportedConversationRecord, StoreError> {
		let NewImportedConversation {
			import_id,
			harness,
			native_conversation,
			working_directory,
			imported_by,
			imported_at_unix_ms,
		} = import;
		let id = import_id.to_string();
		let (actor_kind, actor_id) = imported_by.columns();
		let actor_id = actor_id.to_string();
		sqlx::query!(
			"INSERT INTO imported_conversations
				(import_id, harness, native_conversation, working_directory,
				actor_kind, actor_id, imported_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
			id,
			harness,
			native_conversation,
			working_directory,
			actor_kind,
			actor_id,
			imported_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(ImportedConversationRecord {
			import_id,
			harness,
			native_conversation,
			working_directory,
			imported_by,
			imported_at_unix_ms,
			resumed_as: None,
		})
	}
}

fn read_row(row: Row) -> Result<ImportedConversationRecord, StoreError> {
	Ok(ImportedConversationRecord {
		import_id: parse_uuid("import_id", &row.import_id)?,
		harness: row.harness,
		native_conversation: row.native_conversation,
		working_directory: row.working_directory,
		imported_by: ActorRecord::parse(&row.actor_kind, &row.actor_id)?,
		imported_at_unix_ms: row.imported_at_unix_ms,
		resumed_as: parse_optional_uuid(
			"resumed_as",
			row.resumed_as.as_deref(),
		)?,
	})
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
