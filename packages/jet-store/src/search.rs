//! The Plane-local Search index (ADR-0036). Documents are a projection of
//! committed semantic Events that the core derives and the store holds in
//! one FTS5 table beside the journal. The index is never an authority:
//! every row can be derived again from the journal, and the position it
//! has reached is what lets an interrupted indexer resume (ADR-0078).

use uuid::Uuid;

use sqlx::SqliteConnection;

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
	rowid: i64,
	conversation_id: String,
	sequence: i64,
	field: String,
	excerpt: String,
	score: f64,
}

#[derive(Clone, Copy)]
enum NameDocuments {
	Include,
	Exclude,
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
		self.search_filtered(terms, limit, NameDocuments::Include)
			.await
	}

	/// The same bounded search without name documents, for a peer whose
	/// negotiated protocol predates independent entity names. The exclusion
	/// happens before ranking and limiting so hidden matches cannot crowd out
	/// fields that peer understands (ADR-0019).
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the index cannot be read.
	pub async fn legacy_search(
		&mut self,
		terms: &[String],
		limit: usize,
	) -> Result<Vec<SearchHitRecord>, StoreError> {
		self.search_filtered(terms, limit, NameDocuments::Exclude)
			.await
	}

	async fn search_filtered(
		&mut self,
		terms: &[String],
		limit: usize,
		name_documents: NameDocuments,
	) -> Result<Vec<SearchHitRecord>, StoreError> {
		let Some(expression) = match_expression(terms) else {
			return Ok(Vec::new());
		};
		let bounded_limit = limit.min(SEARCH_HIT_LIMIT);
		let sql_limit = i64::try_from(bounded_limit).unwrap_or(i64::MAX);
		// The compile-time macro cannot describe a MATCH against an FTS5
		// virtual table: sqlx 0.9.0 crashes while inferring its column types,
		// so this one statement runs on the runtime API and names every
		// column it reads. The table and the bound parameters are otherwise
		// the same as the checked statements around it.
		let mut rows = search_table(
			self.connection(),
			CONTENT_SEARCH,
			&expression,
			sql_limit,
		)
		.await?;
		if matches!(name_documents, NameDocuments::Include) {
			rows.extend(
				search_table(
					self.connection(),
					NAME_SEARCH,
					&expression,
					sql_limit,
				)
				.await?,
			);
		}
		rows.sort_by(|left, right| {
			left.score
				.total_cmp(&right.score)
				.then_with(|| right.sequence.cmp(&left.sequence))
				.then_with(|| left.field.cmp(&right.field))
				.then_with(|| left.rowid.cmp(&right.rowid))
		});
		rows.truncate(bounded_limit);
		rows.into_iter().map(read_hit_row).collect()
	}
}

impl WriteTransaction {
	/// Rebuilds one bounded batch of name documents for retained Events that an
	/// older release may have advanced the shared index past. This covers both
	/// legacy creations and name changes that a rollback release could not
	/// project. It is idempotent and advances its own watermark per batch.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the projection cannot be reconciled.
	pub async fn reconcile_name_documents(
		&mut self,
	) -> Result<usize, StoreError> {
		let reconciled_through = sqlx::query_scalar!(
			"SELECT reconciled_through_sequence
			 FROM search_name_index_state WHERE singleton = 1"
		)
		.fetch_one(self.connection())
		.await?;
		let indexed_through =
			sequence_column(self.search_index_position().await?)?;
		if reconciled_through >= indexed_through {
			return Ok(0);
		}
		let limit = i64::try_from(SEARCH_INDEX_BATCH_LIMIT).unwrap_or(i64::MAX);
		let sequences = sqlx::query_scalar!(
			"SELECT sequence FROM events
			 WHERE sequence > ?1 AND sequence <= ?2
			 ORDER BY sequence LIMIT ?3",
			reconciled_through,
			indexed_through,
			limit,
		)
		.fetch_all(self.connection())
		.await?;
		let traversed = sequences.len();
		let batch_through = if traversed < SEARCH_INDEX_BATCH_LIMIT {
			indexed_through
		} else {
			sequences.last().copied().unwrap_or(indexed_through)
		};
		// Current creation Events already have a document by the time this runs.
		// A skipped name-change Event carries its own historical value; using the
		// entity's latest row would collapse several rollback-era renames into
		// duplicate, falsely referenced hits.
		sqlx::query!(
			"INSERT INTO search_name_documents (
				conversation_id, sequence, field, body
			)
			SELECT c.conversation_id, e.sequence, 'name', CASE e.kind
				WHEN 'conversation.name_changed' THEN
					json_extract(e.payload, '$.name.value')
				ELSE 'Conversation ' || substr(
					replace(c.conversation_id, '-', ''), 1, 8
				)
			END
			FROM events AS e
			JOIN conversations AS c
				ON c.conversation_id = e.conversation_id
			WHERE e.kind IN (
				'conversation.created', 'conversation.name_changed'
			)
				AND e.sequence > ?1 AND e.sequence <= ?2
				AND NOT EXISTS (
				SELECT 1 FROM search_name_documents AS d
				WHERE d.conversation_id = c.conversation_id
					AND d.sequence = e.sequence
					AND d.field = 'name'
			)",
			reconciled_through,
			batch_through,
		)
		.execute(self.connection())
		.await?;
		sqlx::query!(
			"INSERT INTO search_name_documents (
				conversation_id, sequence, field, body
			)
			SELECT r.conversation_id, e.sequence, 'name', CASE e.kind
				WHEN 'run.name_changed' THEN
					json_extract(e.payload, '$.name.value')
				ELSE 'Run ' || substr(replace(r.run_id, '-', ''), 1, 8)
			END
			FROM events AS e
			JOIN runs AS r
				ON r.conversation_id = e.conversation_id
				AND r.run_id = e.run_id
			WHERE e.kind IN ('run.created', 'run.name_changed')
				AND e.sequence > ?1 AND e.sequence <= ?2
				AND NOT EXISTS (
				SELECT 1 FROM search_name_documents AS d
				WHERE d.conversation_id = r.conversation_id
					AND d.sequence = e.sequence
					AND d.field = 'name'
			)",
			reconciled_through,
			batch_through,
		)
		.execute(self.connection())
		.await?;
		sqlx::query!(
			"UPDATE search_name_index_state
			 SET reconciled_through_sequence = ?1 WHERE singleton = 1",
			batch_through,
		)
		.execute(self.connection())
		.await?;
		Ok(traversed)
	}

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
			if document.field == "name" {
				sqlx::query!(
					"INSERT INTO search_name_documents (
						conversation_id, sequence, field, body
					) VALUES (?1, ?2, ?3, ?4)",
					conversation_id,
					sequence,
					document.field,
					document.body
				)
				.execute(self.connection())
				.await?;
			} else {
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
		let content = sqlx::query!(
			"DELETE FROM search_documents WHERE conversation_id = ?1",
			conversation_id
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		let names = sqlx::query!(
			"DELETE FROM search_name_documents WHERE conversation_id = ?1",
			conversation_id
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		Ok(content.saturating_add(names))
	}
}

const CONTENT_SEARCH: &str = "SELECT rowid, conversation_id, sequence, field,
	bm25(search_documents) AS score,
	snippet(search_documents, 3, '', '', '…', ?2) AS excerpt
	FROM search_documents
	WHERE search_documents MATCH ?1
	ORDER BY rank, sequence DESC, rowid
	LIMIT ?3";

const NAME_SEARCH: &str = "SELECT rowid, conversation_id, sequence, field,
	bm25(search_name_documents) AS score,
	snippet(search_name_documents, 3, '', '', '…', ?2) AS excerpt
	FROM search_name_documents
	WHERE search_name_documents MATCH ?1
	ORDER BY rank, sequence DESC, rowid
	LIMIT ?3";

async fn search_table(
	connection: &mut SqliteConnection,
	query: &'static str,
	expression: &str,
	limit: i64,
) -> Result<Vec<HitRow>, StoreError> {
	Ok(sqlx::query_as(query)
		.bind(expression)
		.bind(EXCERPT_TOKENS)
		.bind(limit)
		.fetch_all(connection)
		.await?)
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
