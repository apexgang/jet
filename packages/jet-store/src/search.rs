//! The Plane-local Search index (ADR-0036). Documents are a projection of
//! committed semantic Events that the core derives and the store holds in
//! one FTS5 table beside the journal. The index is never an authority:
//! every row can be derived again from the journal, and the position it
//! has reached is what lets an interrupted indexer resume (ADR-0078).

use crate::{
	StoreError,
	journal::{read_event_row, sequence_column},
	records::{EventRecord, column_error, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use sqlx::SqliteConnection;
use uuid::Uuid;

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
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		NewSearchDocument, SEARCH_DOCUMENT_BODY_LIMIT, SEARCH_HIT_LIMIT,
		SEARCH_INDEX_BATCH_LIMIT, SearchHitRecord,
	};
	use crate::{
		ActorRecord, ConversationOriginRecord, EventClass, NewConversation,
		NewEvent, RetentionPolicy, Store, StoreError, VerifiedSnapshotCoverage,
		WorkingTreeRecord,
	};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	async fn open(dir: &tempfile::TempDir) -> Store {
		Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap()
	}

	fn event(conversation_id: Uuid, class: EventClass) -> NewEvent {
		NewEvent {
			event_id: Uuid::now_v7(),
			actor: ActorRecord::InteractiveClient {
				client_id: Uuid::nil(),
			},
			recorded_at_unix_ms: NOW_UNIX_MS,
			conversation_id: Some(conversation_id),
			run_id: None,
			kind: "workspace.created".into(),
			payload_version: 1,
			payload: "{}".into(),
			class,
		}
	}

	fn conversation_event(conversation_id: Uuid, kind: &str) -> NewEvent {
		NewEvent {
			kind: kind.into(),
			..event(conversation_id, EventClass::Semantic)
		}
	}

	async fn conversation(store: &Store) -> Uuid {
		let conversation_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await
			})
			.await
			.unwrap();
		conversation_id
	}

	/// Appends one semantic Event and returns its sequence.
	async fn append(
		store: &Store,
		conversation_id: Uuid,
		class: EventClass,
	) -> u64 {
		store
			.write(async |tx| {
				Ok::<_, StoreError>(
					tx.append_event(event(conversation_id, class))
						.await?
						.sequence,
				)
			})
			.await
			.unwrap()
	}

	fn document(
		conversation_id: Uuid,
		sequence: u64,
		body: &str,
	) -> NewSearchDocument {
		NewSearchDocument {
			conversation_id,
			sequence,
			field: "path".into(),
			body: body.into(),
		}
	}

	fn name_document(
		conversation_id: Uuid,
		sequence: u64,
		body: &str,
	) -> NewSearchDocument {
		NewSearchDocument {
			conversation_id,
			sequence,
			field: "name".into(),
			body: body.into(),
		}
	}

	async fn index(
		store: &Store,
		documents: Vec<NewSearchDocument>,
		through_sequence: u64,
	) -> Result<(), StoreError> {
		store
			.write(async |tx| {
				tx.index_search_documents(documents, through_sequence).await
			})
			.await
	}

	async fn search(
		store: &Store,
		terms: &[&str],
		limit: usize,
	) -> Vec<SearchHitRecord> {
		let terms: Vec<String> =
			terms.iter().map(ToString::to_string).collect();
		store
			.read(async |tx| tx.search(&terms, limit).await)
			.await
			.unwrap()
	}

	async fn position(store: &Store) -> u64 {
		store
			.read(async |tx| tx.search_index_position().await)
			.await
			.unwrap()
	}

	#[tokio::test]
	async fn hits_are_ranked_bounded_and_reference_their_event() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let first = conversation(&store).await;
		let second = conversation(&store).await;
		let first_sequence = append(&store, first, EventClass::Semantic).await;
		let second_sequence =
			append(&store, second, EventClass::Semantic).await;
		let third_sequence = append(&store, second, EventClass::Semantic).await;
		index(
			&store,
			vec![
				document(first, first_sequence, "src/search/index.rs"),
				document(second, second_sequence, "docs/search.md"),
				document(second, third_sequence, "README.md"),
			],
			third_sequence,
		)
		.await
		.unwrap();

		let all = search(&store, &["search"], 10).await;
		let bounded = search(&store, &["search"], 1).await;

		assert_eq!(
			all,
			vec![
				SearchHitRecord {
					conversation_id: second,
					sequence: second_sequence,
					field: "path".into(),
					excerpt: "docs/search.md".into(),
				},
				SearchHitRecord {
					conversation_id: first,
					sequence: first_sequence,
					field: "path".into(),
					excerpt: "src/search/index.rs".into(),
				},
			]
		);
		assert_eq!(bounded, all[..1].to_vec());
		assert_eq!(position(&store).await, third_sequence);
	}

	/// A client predating names must not let highly-ranked name matches consume
	/// the bounded result before an older field is considered.
	#[tokio::test]
	async fn legacy_search_excludes_names_before_ranking_and_limiting() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let path_sequence =
			append(&store, conversation_id, EventClass::Semantic).await;
		let mut documents =
			vec![document(conversation_id, path_sequence, "needle")];
		let mut through_sequence = path_sequence;
		for _ in 0..SEARCH_HIT_LIMIT {
			through_sequence =
				append(&store, conversation_id, EventClass::Semantic).await;
			documents.push(name_document(
				conversation_id,
				through_sequence,
				"needle",
			));
		}
		index(&store, documents, through_sequence).await.unwrap();

		let terms = vec!["needle".to_owned()];
		let (current, legacy, names_in_rollback_table) = store
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.search(&terms, SEARCH_HIT_LIMIT).await?,
					tx.legacy_search(&terms, SEARCH_HIT_LIMIT).await?,
					sqlx::query_scalar::<_, i64>(
						"SELECT COUNT(*) FROM search_documents WHERE field = 'name'",
					)
					.fetch_one(tx.connection())
					.await?,
				))
			})
			.await
			.unwrap();

		assert_eq!(current.len(), SEARCH_HIT_LIMIT);
		assert!(current.iter().all(|hit| hit.field == "name"));
		assert_eq!(names_in_rollback_table, 0);
		assert_eq!(
			legacy,
			vec![SearchHitRecord {
				conversation_id,
				sequence: path_sequence,
				field: "path".into(),
				excerpt: "needle".into(),
			}]
		);
	}

	#[tokio::test]
	async fn name_reconciliation_is_idempotent_and_advances_its_watermark() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let (sequence, first_read, second_read) = store
			.write(async |tx| {
				let sequence = tx
					.append_event(conversation_event(
						conversation_id,
						"conversation.created",
					))
					.await?
					.sequence;
				tx.index_search_documents(Vec::new(), sequence).await?;
				let first_read = tx.reconcile_name_documents().await?;
				let second_read = tx.reconcile_name_documents().await?;
				Ok::<_, StoreError>((sequence, first_read, second_read))
			})
			.await
			.unwrap();

		let (documents, reconciled_through) = store
			.read(async |tx| {
				Ok::<_, StoreError>((
					sqlx::query_scalar::<_, i64>(
						"SELECT COUNT(*) FROM search_name_documents",
					)
					.fetch_one(tx.connection())
					.await?,
					sqlx::query_scalar::<_, i64>(
						"SELECT reconciled_through_sequence
					 FROM search_name_index_state WHERE singleton = 1",
					)
					.fetch_one(tx.connection())
					.await?,
				))
			})
			.await
			.unwrap();

		assert_eq!(
			(documents, reconciled_through, first_read, second_read),
			(1, sequence as i64, 1, 0)
		);
	}

	#[tokio::test]
	async fn name_reconciliation_advances_in_bounded_event_batches() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let (
			through,
			first_read,
			first_watermark,
			second_read,
			second_watermark,
		) = store
			.write(async |tx| {
				let mut through = 0;
				for _ in 0..=SEARCH_INDEX_BATCH_LIMIT {
					through = tx
						.append_event(conversation_event(
							conversation_id,
							"conversation.created",
						))
						.await?
						.sequence;
				}
				tx.index_search_documents(Vec::new(), through).await?;
				let first_read = tx.reconcile_name_documents().await?;
				let first_watermark = sqlx::query_scalar!(
					"SELECT reconciled_through_sequence
					 FROM search_name_index_state WHERE singleton = 1"
				)
				.fetch_one(tx.connection())
				.await?;
				let second_read = tx.reconcile_name_documents().await?;
				let second_watermark = sqlx::query_scalar!(
					"SELECT reconciled_through_sequence
					 FROM search_name_index_state WHERE singleton = 1"
				)
				.fetch_one(tx.connection())
				.await?;
				Ok::<_, StoreError>((
					through,
					first_read,
					first_watermark,
					second_read,
					second_watermark,
				))
			})
			.await
			.unwrap();

		assert_eq!(first_read, SEARCH_INDEX_BATCH_LIMIT);
		assert!(first_watermark < through as i64);
		assert_eq!((second_read, second_watermark), (1, through as i64));
	}

	/// A term is matched as content, never read as FTS5 syntax: operators and
	/// column filters find nothing instead of widening the search, and an
	/// unbalanced quote is punctuation the tokenizer drops rather than an
	/// error.
	#[tokio::test]
	async fn query_syntax_in_a_term_is_matched_as_plain_text() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let sequence =
			append(&store, conversation_id, EventClass::Semantic).await;
		index(
			&store,
			vec![document(conversation_id, sequence, "src/lib.rs")],
			sequence,
		)
		.await
		.unwrap();

		let widened = search(&store, &["missing OR lib"], 10).await;
		let filtered = search(&store, &["field:path"], 10).await;
		let unbalanced = search(&store, &["\"lib"], 10).await;
		let plain = search(&store, &["lib"], 10).await;

		assert_eq!(
			(widened, filtered, unbalanced.len(), plain.len()),
			(Vec::new(), Vec::new(), 1, 1)
		);
	}

	#[tokio::test]
	async fn no_terms_find_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let sequence =
			append(&store, conversation_id, EventClass::Semantic).await;
		index(
			&store,
			vec![document(conversation_id, sequence, "src/lib.rs")],
			sequence,
		)
		.await
		.unwrap();

		assert_eq!(search(&store, &[], 10).await, Vec::new());
	}

	/// Forgetting a Conversation removes what the index holds of it and
	/// nothing else: the other Conversation's documents, the journal, and the
	/// index position stay as they were (ADR-0011, ADR-0078).
	#[tokio::test]
	async fn removing_a_conversation_leaves_the_journal_and_other_documents() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let forgotten = conversation(&store).await;
		let kept = conversation(&store).await;
		let forgotten_sequence =
			append(&store, forgotten, EventClass::Semantic).await;
		let kept_sequence = append(&store, kept, EventClass::Semantic).await;
		index(
			&store,
			vec![
				document(forgotten, forgotten_sequence, "src/forgotten.rs"),
				document(kept, kept_sequence, "src/kept.rs"),
			],
			kept_sequence,
		)
		.await
		.unwrap();

		let removed = store
			.write(async |tx| tx.remove_search_documents(forgotten).await)
			.await
			.unwrap();
		let (hits, journal, cursor) = store
			.read(async |tx| {
				let hits = tx.search(&["src".into()], 10).await?;
				let (_, journal) = tx.events_after(0, 10).await?;
				let cursor = tx.search_index_position().await?;
				Ok::<_, StoreError>((hits, journal.len(), cursor))
			})
			.await
			.unwrap();

		assert_eq!(
			(removed, hits, journal, cursor),
			(
				1,
				vec![SearchHitRecord {
					conversation_id: kept,
					sequence: kept_sequence,
					field: "path".into(),
					excerpt: "src/kept.rs".into(),
				}],
				2,
				kept_sequence
			)
		);
	}

	/// The index position only moves forward through the journal, so an
	/// indexer that lost its place cannot silently skip or repeat Events.
	#[tokio::test]
	async fn the_index_position_moves_forward_within_the_journal() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let first = append(&store, conversation_id, EventClass::Semantic).await;
		let second =
			append(&store, conversation_id, EventClass::Semantic).await;
		index(&store, Vec::new(), second).await.unwrap();

		let backwards = index(&store, Vec::new(), first).await.unwrap_err();
		let ahead = index(&store, Vec::new(), second + 1).await.unwrap_err();

		assert!(matches!(backwards, StoreError::Integrity(_)), "{backwards}");
		assert!(matches!(ahead, StoreError::Integrity(_)), "{ahead}");
		assert_eq!(position(&store).await, second);
	}

	#[tokio::test]
	async fn an_oversized_document_is_refused_before_anything_is_written() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let sequence =
			append(&store, conversation_id, EventClass::Semantic).await;
		let oversized = "a".repeat(SEARCH_DOCUMENT_BODY_LIMIT + 1);

		let error = index(
			&store,
			vec![document(conversation_id, sequence, &oversized)],
			sequence,
		)
		.await
		.unwrap_err();

		assert!(matches!(error, StoreError::Integrity(_)), "{error}");
		assert_eq!(position(&store).await, 0);
	}

	/// The indexer reads semantic Events only, and reads them past an
	/// operational compaction that moved the replay floor: semantic history
	/// outlives replay frames (ADR-0078).
	#[tokio::test]
	async fn semantic_events_are_read_past_compacted_operational_ones() {
		let dir = tempfile::tempdir().unwrap();
		let store = open(&dir).await;
		let conversation_id = conversation(&store).await;
		let first = append(&store, conversation_id, EventClass::Semantic).await;
		append(&store, conversation_id, EventClass::Operational).await;
		let third = append(&store, conversation_id, EventClass::Semantic).await;
		store
			.write(async |tx| {
				let coverage: VerifiedSnapshotCoverage =
					tx.verified_projection_coverage().await?;
				tx.compact_operational_events(coverage, NOW_UNIX_MS + 1)
					.await
			})
			.await
			.unwrap();

		let (floor, sequences) = store
			.read(async |tx| {
				let (_, floor) = tx.journal_position().await?;
				let events = tx.semantic_events_after(0, 10).await?;
				Ok::<_, StoreError>((
					floor,
					events
						.into_iter()
						.map(|event| event.sequence)
						.collect::<Vec<_>>(),
				))
			})
			.await
			.unwrap();

		assert_eq!((floor, sequences), (first + 1, vec![first, third]));
	}
}
