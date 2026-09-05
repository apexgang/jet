//! Wire form of registered Projects (ADR-0025, ADR-0101).
//!
//! A Project is registered through an explicit Path grant: the one request
//! in this protocol that carries an absolute path. Every other file
//! operation names a Project and a relative path.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capability::ToolAvailability;
use crate::event::Actor;

/// What a Path grant would register, shown before anything is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPreview {
	/// The canonical directory the granted path resolves to.
	pub root: String,
	/// Whether that directory can be a Project, and what it is if so.
	pub registrability: Registrability,
}

/// Whether a granted directory can be a Project. What keeps it from
/// registering is data a client acts on, never a message it parses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Registrability {
	/// An ordinary working tree, described.
	Registrable {
		/// The working tree as the Plane's Git describes it.
		repository: Repository,
	},
	/// Git finds no repository at the directory or above it.
	NotARepository,
	/// The directory carries a `.git` entry that Git cannot open, such as
	/// a linked worktree whose repository is gone.
	BrokenRepository,
	/// A bare repository, which has no working tree for Runs to use.
	BareRepository,
	/// The directory lies inside a repository's own `.git` directory.
	InsideGitDir,
	/// The directory lies inside a working tree without being its top. The
	/// grant is for the directory named; the user may grant the top
	/// instead.
	InsideWorkingTree {
		/// The top of the working tree the directory lies in.
		toplevel: String,
	},
}

/// A registrable working tree as the Plane's Git describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
	/// Whether the working tree is the repository's own or a linked one.
	pub worktree: Worktree,
	/// Whether a sparse checkout narrows it; reported, never changed.
	pub checkout: Checkout,
	/// The submodules its index holds, each as its Git link alone.
	pub submodules: Vec<GitLink>,
	/// Whether the Plane has Git LFS; the Plane never bundles it.
	pub lfs: ToolAvailability,
}

/// Which working tree of its repository a Project is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Worktree {
	/// The repository's own working tree.
	Main,
	/// A linked worktree, sharing the repository at `common_dir`.
	Linked {
		/// The `.git` directory (or bare repository) the worktree shares.
		common_dir: String,
	},
}

/// Whether a working tree is checked out in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checkout {
	/// Every tracked path is present.
	Full,
	/// Sparse checkout narrows which paths are present.
	Sparse,
}

/// One submodule as the index records it: a path holding a commit of
/// another repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitLink {
	/// The path inside the working tree, as Git spells it.
	pub path: String,
	/// The commit the link points at, as Git spells it.
	pub commit: String,
}

/// One entry inside a registered Project, addressed by the Project and a
/// path relative to its root. It carries metadata and never content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
	/// Newest Event sequence visible when the Project was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The Project the path was resolved in.
	pub project_id: Uuid,
	/// The path as it was asked for.
	pub path: String,
	/// What the path names right now.
	pub kind: EntryKind,
}

/// What one path inside a Project names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
	/// A regular file.
	File {
		/// Its length in bytes.
		bytes: u64,
	},
	/// A directory.
	Directory,
	/// Something else the filesystem holds, such as a socket.
	Other,
	/// Nothing yet.
	Missing,
}

/// One registered Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
	/// Durable identity.
	pub project_id: Uuid,
	/// The canonical absolute root of its working tree, as the Plane
	/// resolved the granted path.
	pub root: String,
	/// The interactive Actor whose Path grant registered it.
	pub registered_by: Actor,
	/// When it was registered, in signed Unix milliseconds.
	pub registered_at_unix_ms: i64,
}

/// Every registered Project on one Plane, fenced by a journal cursor
/// (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectList {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The Projects in the order they were registered.
	pub projects: Vec<Project>,
}
