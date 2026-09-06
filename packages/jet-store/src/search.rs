//! The Plane-local Search index (ADR-0036). Documents are a projection of
//! committed semantic Events that the core derives and the store holds in
//! one FTS5 table beside the journal. The index is never an authority:
//! every row can be derived again from the journal, and the position it
//! has reached is what lets an interrupted indexer resume (ADR-0078).

use uuid::Uuid;

use crate::StoreError;
use crate::journal::{read_event_row, sequence_column};
use crate::records::{EventRecord, column_error, parse_uuid};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// Most characters one document body may hold. FTS5 tables take no CHECK
/// constraint, so the bound lives here (ASVS 2.2.1).
pub const SEARCH_DOCUMENT_BODY_LIMIT: usize = 4096;

/// Most hits one search returns, whatever the caller asks for.
pub const SEARCH_HIT_LIMIT: usize = 64;

/// Most Events one indexing batch reads.
pub const SEARCH_INDEX_BATCH_LIMIT: usize = 256;

/// One piece of human-visible content to index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSearchDocument {
	/// The Conversation the content belongs to.
	pub conversation_id: Uuid,
	/// The Event that carried the content.
	pub sequence: u64,
	/// What kind of content it is, in the core's vocabulary.
	pub field: String,
	/// The content itself.
	pub body: String,
}

/// One ranked match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHitRecord {
	/// The Conversation the match belongs to.
	pub conversation_id: Uuid,
	/// The Event that carried the matched content.
	pub sequence: u64,
	/// What kind of content matched.
	pub field: String,
	/// A bounded excerpt of the matched content.
	pub excerpt: String,
}

/// Most tokens an excerpt shows around the match.
const EXCERPT_TOKENS: i64 = 16;

#[derive(sqlx::FromRow)]
struct HitRow {
	conversation_id: String,
	sequence: i64,
	field: String,
	excerpt: String,
}

impl ReadTransaction {
	/// The journal position the index has been derived through.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the position cannot be read.
	pub async fn search_index_position(&mut self) -> Result<u64, StoreError> {
		let position = sqlx::query_scalar!(
			"SELECT indexed_through_sequence FROM search_index_state
			 WHERE singleton = 1"
		)
		.fetch_one(self.connection())
		.await?;
		u64::try_from(position).map_err(|_| {
			column_error(
				"indexed_through_sequence",
				format!("search index position {position} is negative"),
			)
		})
	}

	/// Up to `limit` semantic Events strictly after `cursor`, in sequence
	/// order. Unlike replay, this reads past the replay floor: semantic
	/// Events survive operational compaction (ADR-0078).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn semantic_events_after(
		&mut self,
		cursor: u64,
		limit: usize,
	) -> Result<Vec<EventRecord>, StoreError> {
		let cursor = sequence_column(cursor)?;
		let limit = i64::try_from(limit.min(SEARCH_INDEX_BATCH_LIMIT))
			.unwrap_or(i64::MAX);
		let rows = sqlx::query_as!(
			crate::journal::Row,
			r#"SELECT sequence AS "sequence!", event_id, actor_kind, actor_id,
				recorded_at_unix_ms, conversation_id, run_id, kind,
				payload_version, payload
			 FROM events
			 WHERE sequence > ?1 AND class = 'semantic'
			 ORDER BY sequence
			 LIMIT ?2"#,
			cursor,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_event_row).collect()
	}

	/// Up to `limit` documents matching every one of `terms`, best match
	/// first. Each term is matched as content: FTS5 operators, column
	/// filters, and quotes inside it carry no meaning (ASVS 1.2.4).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the index cannot be read.
	pub async fn search(
		&mut self,
		terms: &[String],
		limit: usize,
	) -> Result<Vec<SearchHitRecord>, StoreError> {
		let Some(expression) = match_expression(terms) else {
			return Ok(Vec::new());
		};
		let limit =
			i64::try_from(limit.min(SEARCH_HIT_LIMIT)).unwrap_or(i64::MAX);
		// The compile-time macro cannot describe a MATCH against an FTS5
		// virtual table: sqlx 0.9.0 crashes while inferring its column types,
		// so this one statement runs on the runtime API and names every
		// column it reads. The table and the bound parameters are otherwise
		// the same as the checked statements around it.
		let rows: Vec<HitRow> = sqlx::query_as(
			"SELECT conversation_id, sequence, field,
				snippet(search_documents, 3, '', '', '…', ?2) AS excerpt
			 FROM search_documents
			 WHERE search_documents MATCH ?1
			 ORDER BY rank, sequence DESC, rowid
			 LIMIT ?3",
		)
		.bind(expression)
		.bind(EXCERPT_TOKENS)
		.bind(limit)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_hit_row).collect()
	}
}

impl WriteTransaction {
	/// Adds `documents` and records that the index now covers the journal
	/// through `through_sequence`, in this one transaction. The position
	/// moves forward only, and never past the journal.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when a body exceeds
	/// [`SEARCH_DOCUMENT_BODY_LIMIT`] or the position would move backwards
	/// or ahead of the journal, and another [`StoreError`] when the rows
	/// cannot be written.
	pub async fn index_search_documents(
		&mut self,
		documents: Vec<NewSearchDocument>,
		through_sequence: u64,
	) -> Result<(), StoreError> {
		let indexed_through = self.search_index_position().await?;
		let high_water = self.event_cursor().await?;
		if through_sequence < indexed_through || through_sequence > high_water {
			return Err(StoreError::Integrity(format!(
				"search index position {through_sequence} is outside \
				 {indexed_through}..={high_water}"
			)));
		}
		for document in documents {
			if document.body.chars().count() > SEARCH_DOCUMENT_BODY_LIMIT {
				return Err(StoreError::Integrity(format!(
					"search document at sequence {} exceeds {SEARCH_DOCUMENT_BODY_LIMIT} characters",
					document.sequence
				)));
			}
			let conversation_id = document.conversation_id.to_string();
			let sequence = sequence_column(document.sequence)?;
			sqlx::query!(
				"INSERT INTO search_documents (
					conversation_id, sequence, field, body
				) VALUES (?1, ?2, ?3, ?4)",
				conversation_id,
				sequence,
				document.field,
				document.body
			)
			.execute(self.connection())
			.await?;
		}
		let through_sequence = sequence_column(through_sequence)?;
		sqlx::query!(
			"UPDATE search_index_state SET indexed_through_sequence = ?1
			 WHERE singleton = 1",
			through_sequence
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Removes every document of one Conversation and returns how many
	/// there were. The journal and the index position are untouched:
	/// forgetting is not compaction (ADR-0011, ADR-0078).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be removed.
	pub async fn remove_search_documents(
		&mut self,
		conversation_id: Uuid,
	) -> Result<u64, StoreError> {
		let conversation_id = conversation_id.to_string();
		Ok(sqlx::query!(
			"DELETE FROM search_documents WHERE conversation_id = ?1",
			conversation_id
		)
		.execute(self.connection())
		.await?
		.rows_affected())
	}
}

/// Builds the FTS5 expression that matches every term as one quoted
/// phrase, or nothing when there is no term to match.
fn match_expression(terms: &[String]) -> Option<String> {
	let phrases: Vec<String> = terms
		.iter()
		.map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
		.collect();
	if phrases.is_empty() {
		None
	} else {
		Some(phrases.join(" "))
	}
}

fn read_hit_row(row: HitRow) -> Result<SearchHitRecord, StoreError> {
	Ok(SearchHitRecord {
		conversation_id: parse_uuid("conversation_id", &row.conversation_id)?,
		sequence: u64::try_from(row.sequence).map_err(|_| {
			column_error(
				"sequence",
				format!(
					"search document sequence {} is negative",
					row.sequence
				),
			)
		})?,
		field: row.field,
		excerpt: row.excerpt,
	})
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
