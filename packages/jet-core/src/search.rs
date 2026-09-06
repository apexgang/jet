//! The Plane-local Search index as clients see it (ADR-0036): bounded,
//! ranked hits over human-visible Conversation content, each one naming
//! the Conversation and the Event that carried the content. The index is
//! derived from the journal and never an authority; how it is kept
//! current lives in `search_index`.

use jet_store::{SEARCH_HIT_LIMIT, SearchHitRecord};

use crate::Core;
use crate::conversation::ConversationId;
use crate::error::CoreError;
use crate::event::EventSequence;
use crate::query::QueryResult;

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
	/// A file path the Conversation touched: its Workspace root or a path
	/// its promotion could not settle.
	Path,
	/// A Git branch the Conversation promoted its Workspace to.
	Branch,
}

impl SearchField {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Path => "path",
			Self::Branch => "branch",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		match text {
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
	core.index_search().await?;
	core.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let indexed_through =
				EventSequence(tx.search_index_position().await?);
			let hits = tx.search(terms.as_slice(), SEARCH_HIT_LIMIT).await?;
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
