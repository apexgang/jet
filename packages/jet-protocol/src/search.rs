//! Wire form of the Plane-local Search index (ADR-0036): bounded ranked
//! hits over human-visible Conversation content, each naming the
//! Conversation and the Event that carried the content. A GUI merges the
//! answers of every Plane it is connected to.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What kind of human-visible content a hit matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
	/// A file path the Conversation touched: its Workspace root or a path
	/// its promotion could not settle.
	Path,
	/// A Git branch the Conversation promoted its Workspace to.
	Branch,
}

/// One ranked match, with the stable reference a client follows to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
	/// The Conversation the content belongs to.
	pub conversation_id: Uuid,
	/// The Event that carried the content, carried as a decimal string
	/// (ADR-0089). A client reads it from the journal or scrolls the
	/// Conversation to it.
	#[serde(with = "crate::decimal")]
	pub sequence: u64,
	/// What kind of content matched.
	pub field: SearchField,
	/// A bounded excerpt of the matched content.
	pub excerpt: String,
}

/// One bounded page of hits, best match first, fenced by the journal
/// position it was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
	/// Newest Event sequence visible when the index was read, carried as a
	/// decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The journal position the index had been derived through, carried
	/// as a decimal string. It equals `cursor` unless indexing was
	/// interrupted since the last Command.
	#[serde(with = "crate::decimal")]
	pub indexed_through: u64,
	/// At most 64 hits, best match first.
	pub hits: Vec<SearchHit>,
}
