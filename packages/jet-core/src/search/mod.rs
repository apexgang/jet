//! The Plane-local Search index as clients see it (ADR-0036): bounded,
//! ranked hits over human-visible Conversation content, each one naming
//! the Conversation and the Event that carried the content. The index is
//! derived from the journal and never an authority; how it is kept
//! current lives in `search_index`.

pub(crate) mod index;

use crate::{
	Core, conversation::ConversationId, error::CoreError, event::EventSequence,
	query::QueryResult,
};
use jet_store::{SEARCH_HIT_LIMIT, SearchHitRecord};

/// Most characters one search text may hold (ASVS 2.2.1).
const SEARCH_TEXT_LIMIT: usize = 256;

/// Most terms one search may combine.
const SEARCH_TERM_LIMIT: usize = 16;

/// What a search asks for: the whitespace-separated terms of one bounded
/// text, every one of which a hit must contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerms(Vec<String>);

impl SearchTerms {
	/// Splits `text` into terms.
	///
	/// # Errors
	///
	/// Returns an `invalid_input` [`CoreError`] when the text is longer
	/// than [`SEARCH_TEXT_LIMIT`], holds more than [`SEARCH_TERM_LIMIT`]
	/// terms, or holds none.
	pub fn parse(text: &str) -> Result<Self, CoreError> {
		if text.chars().count() > SEARCH_TEXT_LIMIT {
			return Err(CoreError::invalid_input(
				"search.text_too_long",
				format!("search text exceeds {SEARCH_TEXT_LIMIT} characters"),
			));
		}
		let terms: Vec<String> =
			text.split_whitespace().map(ToOwned::to_owned).collect();
		if terms.is_empty() {
			return Err(CoreError::invalid_input(
				"search.empty",
				"search text holds no term",
			));
		}
		if terms.len() > SEARCH_TERM_LIMIT {
			return Err(CoreError::invalid_input(
				"search.too_many_terms",
				format!("search text exceeds {SEARCH_TERM_LIMIT} terms"),
			));
		}
		Ok(Self(terms))
	}

	pub(crate) fn as_slice(&self) -> &[String] {
		&self.0
	}
}

/// What kind of human-visible content a document holds. Only content a
/// user sees is indexed: raw terminal bytes, credentials, Pairing
/// secrets, and diagnostic detail never reach the index (ADR-0036).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
	/// A Conversation or Run name.
	Name,
	/// A file path the Conversation touched: its Workspace root or a path
	/// its promotion could not settle.
	Path,
	/// A Git branch the Conversation promoted its Workspace to.
	Branch,
}

impl SearchField {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Name => "name",
			Self::Path => "path",
			Self::Branch => "branch",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		match text {
			"name" => Some(Self::Name),
			"path" => Some(Self::Path),
			"branch" => Some(Self::Branch),
			_ => None,
		}
	}
}

/// One ranked match, with the stable reference a client follows to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
	/// The Conversation the content belongs to.
	pub conversation_id: ConversationId,
	/// The Event that carried the content, which a client reads from the
	/// journal or scrolls the Conversation to.
	pub sequence: EventSequence,
	/// What kind of content matched.
	pub field: SearchField,
	/// A bounded excerpt of the matched content.
	pub excerpt: String,
}

/// One bounded page of hits, best match first, fenced by the journal
/// position it was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
	/// Newest Event sequence visible when the index was read.
	pub cursor: EventSequence,
	/// The journal position the index had been derived through. It equals
	/// `cursor` unless indexing was interrupted since the last Command.
	pub indexed_through: EventSequence,
	/// At most [`SEARCH_HIT_LIMIT`] hits, best match first.
	pub hits: Vec<SearchHit>,
}

impl TryFrom<SearchHitRecord> for SearchHit {
	type Error = CoreError;

	fn try_from(record: SearchHitRecord) -> Result<Self, CoreError> {
		let Some(field) = SearchField::parse(&record.field) else {
			return Err(CoreError::internal(
				"search.unknown_field",
				format!("search document field {:?} is unknown", record.field),
			));
		};
		Ok(Self {
			conversation_id: ConversationId(record.conversation_id),
			sequence: EventSequence(record.sequence),
			field,
			excerpt: record.excerpt,
		})
	}
}

/// Brings the index up to the journal, then reads the hits and the two
/// positions that fence them from one SQLite snapshot (ASVS 2.3.3).
pub(crate) async fn query(
	core: &Core,
	terms: &SearchTerms,
) -> Result<QueryResult, CoreError> {
	query_visible_to(core, terms, NameVisibility::Current).await
}

/// The same result without fields introduced by independent entity names.
pub(crate) async fn legacy_query(
	core: &Core,
	terms: &SearchTerms,
) -> Result<QueryResult, CoreError> {
	query_visible_to(core, terms, NameVisibility::BeforeNames).await
}

#[derive(Clone, Copy)]
enum NameVisibility {
	Current,
	BeforeNames,
}

async fn query_visible_to(
	core: &Core,
	terms: &SearchTerms,
	visibility: NameVisibility,
) -> Result<QueryResult, CoreError> {
	core.index_search().await?;
	if matches!(visibility, NameVisibility::Current) {
		// A current Search includes names, so it waits for the resumable name
		// projection before reporting the shared index fence as complete. Legacy
		// peers neither read nor wait for that post-minor-20 projection.
		core.index_search_names().await?;
	}
	core.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let indexed_through =
				EventSequence(tx.search_index_position().await?);
			let hits = match visibility {
				NameVisibility::Current => {
					tx.search(terms.as_slice(), SEARCH_HIT_LIMIT).await?
				}
				NameVisibility::BeforeNames => {
					tx.legacy_search(terms.as_slice(), SEARCH_HIT_LIMIT).await?
				}
			};
			Ok(QueryResult::Search(SearchResult {
				cursor,
				indexed_through,
				hits: hits
					.into_iter()
					.map(SearchHit::try_from)
					.collect::<Result<_, _>>()?,
			}))
		})
		.await
}

#[cfg(test)]
pub(crate) mod tests {
	use jet_store::{
		ConversationOriginRecord, NameRecord, NameSourceRecord,
		NewConversation, NewRun, SEARCH_INDEX_BATCH_LIMIT, Store,
		WorkingTreeRecord,
	};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::event::EventSubject;
	use crate::test_support::{
		Diverged, actor, diverged, preview_promotion, request, start_core,
	};
	use crate::{
		Command, CommandOutcome, ConversationId, Core, CoreError,
		CredentialSource, EventKind, EventSequence, Name, NameSource,
		PairingGate, PairingMethod, PromotionDestination, ProviderId, Query,
		QueryResult, RetentionPolicy, SearchField, SearchHit, SearchResult,
		SearchTerms, WorkingTree,
	};

	async fn search(core: &Core, text: &str) -> SearchResult {
		let result = core
			.query(
				&actor(),
				Query::Search {
					terms: SearchTerms::parse(text).unwrap(),
				},
			)
			.await
			.unwrap();
		let QueryResult::Search(found) = result else {
			panic!("expected QueryResult::Search");
		};
		found
	}

	/// The sequence of the newest journal Event of `kind`, oldest first.
	async fn sequence_of(core: &Core, kind: &str) -> EventSequence {
		let result = core
			.query(
				&actor(),
				Query::Events {
					after: EventSequence(0),
				},
			)
			.await
			.unwrap();
		let QueryResult::Events(page) = result else {
			panic!("expected QueryResult::Events");
		};
		page.events
			.into_iter()
			.rev()
			.find(|event| event.kind.encode().unwrap().kind == kind)
			.map(|event| event.sequence)
			.unwrap()
	}

	/// A hit names the Conversation and the Event that carried the content:
	/// the Workspace root from the Event that created it, an unsettled path
	/// from the Event that recorded the promotion (ADR-0036).
	#[tokio::test]
	async fn hits_reference_the_conversation_and_the_event_that_carried_them() {
		let dir = tempfile::tempdir().unwrap();
		let Diverged {
			core,
			repository,
			workspace,
			..
		} = diverged(dir.path()).await;
		std::fs::write(repository.join("f.txt"), "X\nb\nC\n").unwrap();
		let preview = preview_promotion(
			&core,
			workspace.workspace_id,
			PromotionDestination::LocalCheckout,
		)
		.await
		.unwrap();
		let outcome = core
			.execute(
				&actor(),
				request(Command::PromoteWorkspace {
					binding: preview.binding,
				}),
			)
			.await
			.unwrap();
		assert!(matches!(
			outcome,
			CommandOutcome::WorkspacePromotionRecorded(_)
		));

		let conflicted = search(&core, "f.txt").await;
		let created = search(&core, "workspaces").await;

		assert_eq!(
			(&conflicted, &created),
			(
				&SearchResult {
					cursor: conflicted.cursor,
					indexed_through: conflicted.cursor,
					hits: vec![SearchHit {
						conversation_id: workspace.conversation_id,
						sequence: sequence_of(
							&core,
							"workspace.promotion_recorded"
						)
						.await,
						field: SearchField::Path,
						excerpt: "f.txt".into(),
					}],
				},
				&SearchResult {
					cursor: conflicted.cursor,
					indexed_through: conflicted.cursor,
					hits: vec![SearchHit {
						conversation_id: workspace.conversation_id,
						sequence: sequence_of(&core, "workspace.created").await,
						field: SearchField::Path,
						excerpt: workspace.root.to_string_lossy().into_owned(),
					}],
				}
			)
		);
	}

	/// What sits beside a Credential or a Pairing secret is never indexed,
	/// however visible its label is elsewhere (ADR-0036, ADR-0076).
	#[tokio::test]
	async fn account_labels_and_pairing_are_not_searchable() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&dir.path().join("plane.sqlite3")).await;
		core.execute(
			&actor(),
			request(Command::BindAccount {
				provider: ProviderId("openai".into()),
				label: "Acme billing".into(),
				provider_account: None,
				credential_source: CredentialSource::PlatformStore,
			}),
		)
		.await
		.unwrap();
		core.execute(
			&actor(),
			request(Command::SetPairingGate {
				gate: PairingGate::Open,
			}),
		)
		.await
		.unwrap();
		core.execute(
			&actor(),
			request(Command::OpenPairing {
				method: PairingMethod::ManualCode,
			}),
		)
		.await
		.unwrap();

		let found = search(&core, "acme").await;

		assert_eq!(
			found,
			SearchResult {
				cursor: found.cursor,
				indexed_through: found.cursor,
				hits: vec![],
			}
		);
		assert!(found.cursor > EventSequence(0));
	}

	/// A daemon that stopped between a Command's commit and its indexing
	/// picks up from the position the index reached, so nothing is missed
	/// and nothing is indexed twice, and the journal is only read (ADR-0078).
	#[tokio::test]
	async fn indexing_resumes_from_where_it_stopped() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let Diverged {
			core, workspace, ..
		} = diverged(dir.path()).await;
		let before = search(&core, "workspaces").await;
		core.close().await;
		drop(core);
		// An Event committed by a Command whose indexing never ran.
		let store = Store::open(&path).await.unwrap();
		let unindexed = store
			.write(async |tx| {
				let event = EventKind::WorkspaceCreated {
					workspace_id: workspace.workspace_id,
					project_id: workspace.project_id,
					root: "/elsewhere/workspaces/interrupted".into(),
					base: workspace.base.clone(),
				}
				.to_record(
					&actor(),
					EventSubject::Conversation(workspace.conversation_id),
					0,
				)?;
				Ok::<_, CoreError>(tx.append_event(event).await?.sequence)
			})
			.await
			.unwrap();
		store.close().await;
		drop(store);

		let core = start_core(&path).await;
		let after = search(&core, "workspaces").await;

		assert_eq!(
			after,
			SearchResult {
				cursor: EventSequence(unindexed),
				indexed_through: EventSequence(unindexed),
				hits: vec![
					SearchHit {
						conversation_id: workspace.conversation_id,
						sequence: EventSequence(unindexed),
						field: SearchField::Path,
						excerpt: "/elsewhere/workspaces/interrupted".into(),
					},
					before.hits[0].clone(),
				],
			}
		);
	}

	/// A rollback release can create a legacy Event after the names migration and
	/// advance the index beyond it. Startup reconciles the deterministic fallback
	/// even though ordinary Event indexing has no work left to read.
	#[tokio::test]
	async fn an_already_indexed_legacy_creation_gets_its_fallback_name() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let conversation_id = ConversationId(
			Uuid::parse_str("12345678-1234-5678-9234-567812345678").unwrap(),
		);
		let run_id = crate::RunId(
			Uuid::parse_str("87654321-1234-5678-9234-567812345678").unwrap(),
		);
		let store = Store::open(&path).await.unwrap();
		let (conversation_sequence, run_sequence) = store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id: conversation_id.0,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: 0,
				})
				.await?;
				let event = EventKind::ConversationCreated {
					name: None,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::NoProject,
					origin: crate::ConversationOrigin::New,
				}
				.to_record(
					&actor(),
					EventSubject::Conversation(conversation_id),
					0,
				)?;
				let conversation_sequence =
					tx.append_event(event).await?.sequence;
				tx.insert_run(NewRun {
					run_id: run_id.0,
					conversation_id: conversation_id.0,
					created_at_unix_ms: 0,
				})
				.await?;
				let event = EventKind::RunCreated { name: None }.to_record(
					&actor(),
					EventSubject::Run {
						conversation_id,
						run_id,
					},
					0,
				)?;
				let run_sequence = tx.append_event(event).await?.sequence;
				tx.index_search_documents(Vec::new(), run_sequence).await?;
				Ok::<_, CoreError>((
					EventSequence(conversation_sequence),
					EventSequence(run_sequence),
				))
			})
			.await
			.unwrap();
		store.close().await;
		drop(store);

		let core = start_core(&path).await;
		let conversation = search(&core, "Conversation 12345678").await;
		let run = search(&core, "Run 87654321").await;

		assert_eq!(
			(conversation.hits, run.hits),
			(
				vec![SearchHit {
					conversation_id,
					sequence: conversation_sequence,
					field: SearchField::Name,
					excerpt: "Conversation 12345678".into(),
				}],
				vec![SearchHit {
					conversation_id,
					sequence: run_sequence,
					field: SearchField::Name,
					excerpt: "Run 87654321".into(),
				}],
			)
		);
	}

	/// A rollback release can advance the shared index past name Events it does not
	/// understand. The next current release reconstructs each historical name from
	/// its Event without collapsing several renames into the entity's latest row.
	#[tokio::test]
	async fn name_changes_skipped_during_rollback_are_reconciled_on_upgrade() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let conversation_id = ConversationId(Uuid::now_v7());
		let run_id = crate::RunId(Uuid::now_v7());
		let store = Store::open(&path).await.unwrap();
		let (conversation_sequences, run_sequences) = store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id: conversation_id.0,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: 0,
				})
				.await?;
				tx.insert_run(NewRun {
					run_id: run_id.0,
					conversation_id: conversation_id.0,
					created_at_unix_ms: 0,
				})
				.await?;
				for number in 0..SEARCH_INDEX_BATCH_LIMIT * 2 {
					let event = EventKind::ConversationNameChanged {
						name: Name {
							value: format!("Historical filler {number}"),
							source: NameSource::Manual,
						},
					}
					.to_record(
						&actor(),
						EventSubject::Conversation(conversation_id),
						0,
					)?;
					tx.append_event(event).await?;
				}
				let mut conversation_sequences = Vec::new();
				for value in ["Conversation Alpha", "Conversation Beta"] {
					tx.update_conversation_name(
						conversation_id.0,
						&NameRecord {
							value: value.into(),
							source: NameSourceRecord::Manual,
						},
					)
					.await?;
					let event = EventKind::ConversationNameChanged {
						name: Name {
							value: value.into(),
							source: NameSource::Manual,
						},
					}
					.to_record(
						&actor(),
						EventSubject::Conversation(conversation_id),
						0,
					)?;
					conversation_sequences.push(EventSequence(
						tx.append_event(event).await?.sequence,
					));
				}
				let mut run_sequences = Vec::new();
				for value in ["Run Alpha", "Run Beta"] {
					tx.update_run_name(
						run_id.0,
						&NameRecord {
							value: value.into(),
							source: NameSourceRecord::Manual,
						},
					)
					.await?;
					let event = EventKind::RunNameChanged {
						name: Name {
							value: value.into(),
							source: NameSource::Manual,
						},
					}
					.to_record(
						&actor(),
						EventSubject::Run {
							conversation_id,
							run_id,
						},
						0,
					)?;
					run_sequences.push(EventSequence(
						tx.append_event(event).await?.sequence,
					));
				}
				// This is the rollback release advancing its old content index while
				// remaining unaware of the separate name projection.
				tx.index_search_documents(Vec::new(), run_sequences[1].0)
					.await?;
				Ok::<_, CoreError>((conversation_sequences, run_sequences))
			})
			.await
			.unwrap();
		store.close().await;
		drop(store);

		let core = start_core(&path).await;
		let hits = [
			search(&core, "Conversation Alpha").await.hits,
			search(&core, "Conversation Beta").await.hits,
			search(&core, "Run Alpha").await.hits,
			search(&core, "Run Beta").await.hits,
		];

		assert_eq!(
			hits,
			[
				vec![SearchHit {
					conversation_id,
					sequence: conversation_sequences[0],
					field: SearchField::Name,
					excerpt: "Conversation Alpha".into(),
				}],
				vec![SearchHit {
					conversation_id,
					sequence: conversation_sequences[1],
					field: SearchField::Name,
					excerpt: "Conversation Beta".into(),
				}],
				vec![SearchHit {
					conversation_id,
					sequence: run_sequences[0],
					field: SearchField::Name,
					excerpt: "Run Alpha".into(),
				}],
				vec![SearchHit {
					conversation_id,
					sequence: run_sequences[1],
					field: SearchField::Name,
					excerpt: "Run Beta".into(),
				}],
			]
		);
	}

	/// Search text is bounded before it reaches the index (ASVS 2.2.1).
	#[test]
	fn search_text_is_bounded() {
		let empty = SearchTerms::parse("  \t ").unwrap_err();
		let long = SearchTerms::parse(&"a".repeat(257)).unwrap_err();
		let many = SearchTerms::parse(&"a ".repeat(17)).unwrap_err();
		let fine = SearchTerms::parse(" src/lib.rs  search ").unwrap();

		assert_eq!(
			(empty.code, long.code, many.code, fine.as_slice()),
			(
				"search.empty".into(),
				"search.text_too_long".into(),
				"search.too_many_terms".into(),
				&["src/lib.rs".to_owned(), "search".to_owned()][..]
			)
		);
	}
}
