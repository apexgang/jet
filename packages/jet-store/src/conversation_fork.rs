//! Destination-owned Fork launch contexts (ADR-0035).

use uuid::Uuid;

use crate::{ReadTransaction, StoreError, WriteTransaction};

struct Row {
	conversation_id: String,
	checkpoint_commit: String,
	checkpoint_tree: String,
	source_craft: Option<String>,
	source_native_conversation: Option<String>,
	context: String,
}

/// Immutable launch material copied into the fork's own lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkLaunchContextRecord {
	/// Destination Conversation that owns this context.
	pub conversation_id: Uuid,
	/// Checked-out commit retained by the selected checkpoint.
	pub checkpoint_commit: String,
	/// Exact working-tree object retained by the selected checkpoint.
	pub checkpoint_tree: String,
	/// Source execution's pinned Craft, when it had one.
	pub source_craft: Option<String>,
	/// Harness-native source identity, when durably observed.
	pub source_native_conversation: Option<String>,
	/// Serialized bounded fork launch context.
	pub context_json: String,
}

impl ReadTransaction {
	/// Reads one fork's immutable, destination-owned launch context.
	///
	/// # Errors
	/// Returns a store error when the query cannot complete.
	pub async fn conversation_fork_launch(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<ForkLaunchContextRecord>, StoreError> {
		let conversation_id = conversation_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT conversation_id AS "conversation_id!",
				checkpoint_commit, checkpoint_tree, source_craft,
				source_native_conversation, context
			 FROM conversation_fork_launches WHERE conversation_id = ?1"#,
			conversation_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(|row| {
			Ok(ForkLaunchContextRecord {
				conversation_id: crate::records::parse_uuid(
					"conversation_id",
					&row.conversation_id,
				)?,
				checkpoint_commit: row.checkpoint_commit,
				checkpoint_tree: row.checkpoint_tree,
				source_craft: row.source_craft,
				source_native_conversation: row.source_native_conversation,
				context_json: row.context,
			})
		})
		.transpose()
	}
}

impl WriteTransaction {
	/// Stores launch material in the destination Conversation's transaction.
	///
	/// # Errors
	/// Returns a store error for an unknown/duplicate destination or an
	/// out-of-bounds value.
	pub async fn insert_conversation_fork_launch(
		&mut self,
		record: &ForkLaunchContextRecord,
	) -> Result<(), StoreError> {
		let conversation_id = record.conversation_id.to_string();
		// ASVS 1.2.4 and 2.3.3: copied provenance remains parameterized and
		// commits atomically with the destination identity.
		sqlx::query!(
			"INSERT INTO conversation_fork_launches
				(conversation_id, checkpoint_commit, checkpoint_tree,
				 source_craft, source_native_conversation, context)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
			conversation_id,
			record.checkpoint_commit,
			record.checkpoint_tree,
			record.source_craft,
			record.source_native_conversation,
			record.context_json,
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}
