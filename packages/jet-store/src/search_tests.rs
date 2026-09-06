use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{NewSearchDocument, SEARCH_DOCUMENT_BODY_LIMIT, SearchHitRecord};
use crate::{
	ActorRecord, EventClass, NewConversation, NewEvent, RetentionPolicy, Store,
	StoreError, VerifiedSnapshotCoverage, WorkingTreeRecord,
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

async fn conversation(store: &Store) -> Uuid {
	let conversation_id = Uuid::now_v7();
	store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
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
	let terms: Vec<String> = terms.iter().map(ToString::to_string).collect();
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
	let second_sequence = append(&store, second, EventClass::Semantic).await;
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

/// A term is matched as content, never read as FTS5 syntax: operators and
/// column filters find nothing instead of widening the search, and an
/// unbalanced quote is punctuation the tokenizer drops rather than an
/// error.
#[tokio::test]
async fn query_syntax_in_a_term_is_matched_as_plain_text() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(&dir).await;
	let conversation_id = conversation(&store).await;
	let sequence = append(&store, conversation_id, EventClass::Semantic).await;
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
	let sequence = append(&store, conversation_id, EventClass::Semantic).await;
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
	let second = append(&store, conversation_id, EventClass::Semantic).await;
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
	let sequence = append(&store, conversation_id, EventClass::Semantic).await;
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
