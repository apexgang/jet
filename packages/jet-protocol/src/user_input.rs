//! Wire forms for direct user edits and submitted file reviews (ADR-0041).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A registered root through which an ordinary file operation is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileTarget {
	/// A registered Project's Local checkout.
	Project {
		/// Project identity.
		project_id: Uuid,
	},
	/// A Conversation-owned Workspace.
	Workspace {
		/// Workspace identity.
		workspace_id: Uuid,
	},
}

/// Exact Git content and mode observed for one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRevision {
	/// Git blob object, or all zeroes when the file is missing.
	pub object: String,
	/// Git mode, or `000000` when the file is missing.
	pub mode: String,
}

/// Bounded UTF-8 content read through a registered root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditableFile {
	/// Journal fence observed with the registered target.
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Registered root that was read.
	pub target: FileTarget,
	/// Validated path relative to that root.
	pub path: String,
	/// Exact optimistic-concurrency precondition for an edit.
	pub revision: FileRevision,
	/// UTF-8 content, or `None` when the path does not exist yet.
	pub content: Option<String>,
}

/// One submitted inline review comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
	/// Validated path relative to the Conversation's working tree.
	pub path: String,
	/// One-based line number in the reviewed file.
	pub line: u32,
	/// User-authored comment text.
	pub comment: String,
}
