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

use crate::{
	StoreError,
	records::{ActorRecord, parse_optional_uuid, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use uuid::Uuid;

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
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{ImportedConversationRecord, NewImportedConversation};
	use crate::{
		ActorRecord, ConversationOriginRecord, NewConversation,
		RetentionPolicy, Store, StoreError, WorkingTreeRecord,
	};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn import(
		harness: &str,
		native_conversation: &str,
	) -> NewImportedConversation {
		NewImportedConversation {
			import_id: Uuid::now_v7(),
			harness: harness.into(),
			native_conversation: native_conversation.into(),
			working_directory: Some("/home/jet/repo".into()),
			imported_by: ActorRecord::InteractiveClient {
				client_id: Uuid::nil(),
			},
			imported_at_unix_ms: NOW_UNIX_MS,
		}
	}

	fn recorded(
		import: &NewImportedConversation,
		resumed_as: Option<Uuid>,
	) -> ImportedConversationRecord {
		ImportedConversationRecord {
			import_id: import.import_id,
			harness: import.harness.clone(),
			native_conversation: import.native_conversation.clone(),
			working_directory: import.working_directory.clone(),
			imported_by: import.imported_by,
			imported_at_unix_ms: import.imported_at_unix_ms,
			resumed_as,
		}
	}

	fn continuing(import_id: Uuid) -> NewConversation {
		NewConversation {
			conversation_id: Uuid::now_v7(),
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTreeRecord::NoProject,
			origin: ConversationOriginRecord::Imported { import_id },
			created_at_unix_ms: NOW_UNIX_MS,
		}
	}

	/// An import outlives the daemon that registered it, is found by its
	/// identity or its Harness-native identity, and reports the Conversation
	/// that continues it once one exists (ADR-0010).
	#[tokio::test]
	async fn imports_survive_reopening_and_report_their_continuation() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let resumed = import("codex", "thread-1");
		let waiting = NewImportedConversation {
			working_directory: None,
			..import("claude-code", "session-2")
		};
		let continuation = continuing(resumed.import_id);

		let first = Store::open(&path).await.unwrap();
		first
			.write(async |tx| {
				tx.insert_imported_conversation(resumed.clone()).await?;
				tx.insert_imported_conversation(waiting.clone()).await?;
				tx.insert_conversation(continuation).await
			})
			.await
			.unwrap();
		first.close().await;

		let second = Store::open(&path).await.unwrap();
		let (listed, by_id, by_identity, unknown, conversation) = second
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.imported_conversations().await?,
					tx.imported_conversation(waiting.import_id).await?,
					tx.imported_conversation_by_identity("codex", "thread-1")
						.await?,
					tx.imported_conversation_by_identity("codex", "thread-9")
						.await?,
					tx.conversation(continuation.conversation_id).await?,
				))
			})
			.await
			.unwrap();

		assert_eq!(
			(
				listed,
				by_id,
				by_identity,
				unknown,
				conversation.map(|c| c.origin)
			),
			(
				vec![
					recorded(&resumed, Some(continuation.conversation_id)),
					recorded(&waiting, None),
				],
				Some(recorded(&waiting, None)),
				Some(recorded(&resumed, Some(continuation.conversation_id))),
				None,
				Some(ConversationOriginRecord::Imported {
					import_id: resumed.import_id,
				}),
			)
		);
	}

	/// One Harness-native identity is one import, and one import is continued
	/// by one Conversation. The core checks before it inserts; the schema
	/// refuses either second row even so.
	#[tokio::test]
	async fn an_identity_is_imported_once_and_continued_once() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let first = import("codex", "thread-1");
		let again = import("codex", "thread-1");
		let elsewhere = import("claude-code", "thread-1");
		let continuation = continuing(first.import_id);

		store
			.write(async |tx| {
				tx.insert_imported_conversation(first.clone()).await?;
				tx.insert_imported_conversation(elsewhere.clone()).await?;
				tx.insert_conversation(continuation).await
			})
			.await
			.unwrap();
		let refused_import = store
			.write(async |tx| tx.insert_imported_conversation(again).await)
			.await
			.unwrap_err();
		let refused_continuation = store
			.write(async |tx| {
				tx.insert_conversation(continuing(first.import_id)).await
			})
			.await
			.unwrap_err();
		let listed = store
			.read(async |tx| tx.imported_conversations().await)
			.await
			.unwrap();

		assert_eq!(
			(
				matches!(refused_import, StoreError::Integrity(_)),
				matches!(refused_continuation, StoreError::Integrity(_)),
				listed,
			),
			(
				true,
				true,
				vec![
					recorded(&first, Some(continuation.conversation_id)),
					recorded(&elsewhere, None),
				]
			)
		);
	}
}
