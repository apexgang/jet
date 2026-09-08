//! Current state of Conversations, independent of any Run (ADR-0001).

use uuid::Uuid;

use crate::StoreError;
use crate::records::{
	ConversationOriginColumns, ConversationOriginRecord, ConversationPageKey,
	ConversationPageStart, ConversationRecord, NameRecord, NameSourceRecord,
	NewConversation, RetentionPolicy, WorkingTreeRecord, column_error,
	parse_name, parse_optional_uuid, parse_uuid,
};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// Most Conversations returned by one keyset page.
pub const CONVERSATION_PAGE_LIMIT: usize = 256;

/// One `conversations` row as SQLite stores it, before its text columns are
/// parsed back into domain types.
struct Row {
	conversation_id: String,
	revision: i64,
	retention: String,
	working_tree: String,
	project_id: Option<String>,
	import_id: Option<String>,
	fork_source_conversation_id: Option<String>,
	fork_source_run_id: Option<String>,
	fork_checkpoint_turn: Option<i64>,
	name: Option<String>,
	name_source: Option<String>,
	created_at_unix_ms: i64,
}

impl ReadTransaction {
	/// The Conversation identified by `conversation_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn conversation(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<ConversationRecord>, StoreError> {
		let conversation_id = conversation_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT conversation_id AS "conversation_id!", revision, retention,
				working_tree, project_id, import_id,
				fork_source_conversation_id, fork_source_run_id,
				fork_checkpoint_turn, name, name_source,
				created_at_unix_ms
			 FROM conversations
			 WHERE conversation_id = ?1"#,
			conversation_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}

	/// Every recorded Conversation in creation order.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn conversations(
		&mut self,
	) -> Result<Vec<ConversationRecord>, StoreError> {
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT conversation_id AS "conversation_id!", revision, retention,
				working_tree, project_id, import_id,
				fork_source_conversation_id, fork_source_run_id,
				fork_checkpoint_turn, name, name_source,
				created_at_unix_ms
			 FROM conversations
			 ORDER BY rowid"#
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// One bounded keyset page of Conversations in creation order.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn conversation_page(
		&mut self,
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
		let limit =
			i64::try_from(CONVERSATION_PAGE_LIMIT + 1).unwrap_or(i64::MAX);
		let rows = sqlx::query!(
			r#"SELECT rowid AS "rowid!",
				conversation_id AS "conversation_id!", revision, retention,
				working_tree, project_id, import_id,
				fork_source_conversation_id, fork_source_run_id,
				fork_checkpoint_turn, name, name_source,
				created_at_unix_ms
			 FROM conversations
			 WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"#,
			after,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		let mut rows = rows
			.into_iter()
			.map(|row| {
				Ok((
					ConversationPageKey(row.rowid),
					read_row(Row {
						conversation_id: row.conversation_id,
						revision: row.revision,
						retention: row.retention,
						working_tree: row.working_tree,
						project_id: row.project_id,
						import_id: row.import_id,
						fork_source_conversation_id: row
							.fork_source_conversation_id,
						fork_source_run_id: row.fork_source_run_id,
						fork_checkpoint_turn: row.fork_checkpoint_turn,
						name: row.name,
						name_source: row.name_source,
						created_at_unix_ms: row.created_at_unix_ms,
					})?,
				))
			})
			.collect::<Result<Vec<_>, StoreError>>()?;
		let next = (rows.len() > CONVERSATION_PAGE_LIMIT)
			.then(|| rows[CONVERSATION_PAGE_LIMIT - 1].0);
		rows.truncate(CONVERSATION_PAGE_LIMIT);
		Ok((rows.into_iter().map(|(_, record)| record).collect(), next))
	}
}

impl WriteTransaction {
	/// Records a new Conversation with no Runs.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the identity is already taken.
	pub async fn insert_conversation(
		&mut self,
		conversation: NewConversation,
	) -> Result<ConversationRecord, StoreError> {
		let record = ConversationRecord {
			conversation_id: conversation.conversation_id,
			revision: 1,
			retention: conversation.retention,
			working_tree: conversation.working_tree,
			origin: conversation.origin,
			name: fallback_name(conversation.conversation_id),
			created_at_unix_ms: conversation.created_at_unix_ms,
		};
		let conversation_id = record.conversation_id.to_string();
		let retention = record.retention.as_str();
		let (working_tree, project_id) = record.working_tree.columns();
		let project_id = project_id.map(|project_id| project_id.to_string());
		let ConversationOriginColumns {
			import_id,
			fork_source_conversation_id,
			fork_source_run_id,
			fork_checkpoint_turn,
		} = record.origin.columns();
		let import_id = import_id.map(|id| id.to_string());
		let fork_source_conversation_id =
			fork_source_conversation_id.map(|id| id.to_string());
		let fork_source_run_id = fork_source_run_id.map(|id| id.to_string());
		let name_source = record.name.source.as_str();
		let revision = i64::try_from(record.revision).unwrap_or(i64::MAX);
		// ASVS 1.2.4: provenance values remain bound parameters; clients never
		// contribute SQL or column names.
		sqlx::query!(
			"INSERT INTO conversations
				(conversation_id, revision, retention, working_tree, project_id,
					import_id, fork_source_conversation_id,
					fork_source_run_id, fork_checkpoint_turn,
					name, name_source, created_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
			conversation_id,
			revision,
			retention,
			working_tree,
			project_id,
			import_id,
			fork_source_conversation_id,
			fork_source_run_id,
			fork_checkpoint_turn,
			record.name.value,
			name_source,
			record.created_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(record)
	}

	/// Replaces one Conversation's resolved name and advances its Revision.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn update_conversation_name(
		&mut self,
		conversation_id: Uuid,
		name: &NameRecord,
	) -> Result<(), StoreError> {
		let conversation_id = conversation_id.to_string();
		let source = name.source.as_str();
		// ASVS 1.2.4: display text remains a bound value, never SQL source.
		sqlx::query!(
			"UPDATE conversations SET name = ?2, name_source = ?3,
				revision = revision + 1
			 WHERE conversation_id = ?1",
			conversation_id,
			name.value,
			source
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

fn read_row(row: Row) -> Result<ConversationRecord, StoreError> {
	let conversation_id = parse_uuid("conversation_id", &row.conversation_id)?;
	Ok(ConversationRecord {
		conversation_id,
		revision: parse_revision(row.revision)?,
		retention: parse_retention(&row.retention)?,
		working_tree: WorkingTreeRecord::parse(
			&row.working_tree,
			parse_optional_uuid("project_id", row.project_id.as_deref())?,
		)?,
		origin: ConversationOriginRecord::parse(ConversationOriginColumns {
			import_id: parse_optional_uuid(
				"import_id",
				row.import_id.as_deref(),
			)?,
			fork_source_conversation_id: parse_optional_uuid(
				"fork_source_conversation_id",
				row.fork_source_conversation_id.as_deref(),
			)?,
			fork_source_run_id: parse_optional_uuid(
				"fork_source_run_id",
				row.fork_source_run_id.as_deref(),
			)?,
			fork_checkpoint_turn: row.fork_checkpoint_turn,
		})?,
		name: parse_name(
			row.name,
			row.name_source.as_deref(),
			fallback_name(conversation_id),
		)?,
		created_at_unix_ms: row.created_at_unix_ms,
	})
}

fn fallback_name(id: Uuid) -> NameRecord {
	NameRecord {
		value: format!("Conversation {}", &id.simple().to_string()[..8]),
		source: NameSourceRecord::Deterministic,
	}
}

fn parse_revision(revision: i64) -> Result<u64, StoreError> {
	u64::try_from(revision).map_err(|_| {
		column_error(
			"revision",
			format!("conversation revision {revision} is negative"),
		)
	})
}

fn parse_retention(text: &str) -> Result<RetentionPolicy, StoreError> {
	RetentionPolicy::parse(text).ok_or_else(|| {
		column_error("retention", format!("unknown retention value {text:?}"))
	})
}
