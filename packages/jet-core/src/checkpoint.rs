//! Immutable turn boundaries and evidence-backed diffs (ADR-0030, ADR-0084).
use crate::{ConversationId, PlaneId, RunId, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Which immutable boundaries a diff compares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffScope {
	/// Run baseline compared with the working tree now.
	Current,
	/// Run baseline compared with its final captured boundary; terminal Runs only.
	Final,
	/// Compare two retained boundaries; zero selects the Run baseline.
	Historical {
		/// Earlier boundary.
		from_turn: u32,
		/// Later boundary.
		to_turn: u32,
	},
	/// Changes in one completed or interrupted turn, numbered from one.
	Turn {
		/// Turn number within the Run.
		turn: u32,
	},
}
/// Attribution justified by exact content and durable activity evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeOrigin {
	/// An authenticated direct edit through Jet.
	UserEdit {
		/// Client responsible for the edit.
		client_id: crate::ClientId,
	},
	/// An operation in a Jet-owned Workspace terminal.
	WorkspaceTerminal {
		/// Durable terminal identity.
		terminal_id: uuid::Uuid,
	},
	/// A correlated Harness file operation.
	Harness {
		/// Run responsible for the operation.
		run_id: RunId,
	},
	/// A complete chain contains more than one origin.
	Mixed,
	/// No activity evidence explains the complete content transition.
	ExternalOrUnknown,
}
/// Durable operation evidence, validated against Git content at the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvidence {
	/// Command, terminal operation, or native Harness activity identity.
	pub activity_id: String,
	/// Source derived by the trusted Adapter, never from filesystem timing.
	pub origin: ChangeOrigin,
	/// Validated repository-relative path.
	pub path: String,
	/// Git object before the operation; all zeros for an addition.
	pub before_object: String,
	/// Git object after the operation; all zeros for a deletion.
	pub after_object: String,
	/// Original Git mode.
	pub before_mode: String,
	/// Resulting Git mode.
	pub after_mode: String,
}
/// Whether complete Artifact bytes were retained within ingestion limits.
#[derive(
	Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactAvailability {
	/// Complete bytes are stored under the advertised hash.
	#[default]
	Stored,
	/// Only hash and size were retained because the Run budget was exhausted.
	RunBudgetExceeded,
	/// Only hash and size were retained because the patch exceeded the size limit.
	ArtifactSizeExceeded,
}
/// An immutable payload under Jet's SHA-256 Artifact namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeArtifact {
	/// Content availability; metadata-only references must not be treated as empty patches.
	#[serde(default)]
	pub availability: ArtifactAvailability,
	/// Lowercase SHA-256 content address.
	pub sha256: String,
	/// Full payload length in bytes.
	pub size: u64,
}
/// Metadata for a Workspace file excluded from content ingestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmittedFile {
	/// Repository-relative path.
	pub path: String,
	/// Observed bytes; content was not read.
	pub size: u64,
	/// Observed Git file mode.
	pub mode: String,
}
/// Git state at an observed boundary, independent of Harness commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSnapshot {
	/// Oversized paths whose live content is not represented by the tree.
	#[serde(default)]
	pub omitted_files: Vec<OmittedFile>,
	/// Checked-out commit at capture time.
	pub commit: String,
	/// Captured working-tree object, including uncommitted changes.
	pub tree: String,
	/// Binary-capable patch from commit to working tree.
	pub uncommitted: ChangeArtifact,
}
/// One changed path and the Git content that establishes its change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
	/// Repository-relative path; renames appear as deletion and addition.
	pub path: String,
	/// Observed size of omitted original content, when available.
	pub before_size: Option<u64>,
	/// Observed size of omitted resulting content, when available.
	pub after_size: Option<u64>,
	/// Original Git object, zeros for additions; absent when content was omitted.
	pub before_object: Option<String>,
	/// Resulting Git object, zeros for deletions; absent when content was omitted.
	pub after_object: Option<String>,
	/// Original Git mode, zero for additions.
	pub before_mode: String,
	/// Resulting Git mode, zero for deletions.
	pub after_mode: String,
	/// Evidence-backed attribution, never derived from timestamps.
	pub origin: ChangeOrigin,
}
/// Terminal outcome of one turn, independent of Run lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
	/// The Harness completed its turn.
	Completed,
	/// The turn ended early with partial changes preserved.
	Interrupted,
}
/// Immutable turn record. Large payloads stay outside SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeCheckpoint {
	/// Durable operation receipts for the gap before this turn began.
	#[serde(default)]
	pub before_evidence: Vec<ChangeEvidence>,
	/// Gap receipts conflicted or exceeded the bound.
	#[serde(default)]
	pub before_evidence_incomplete: bool,
	/// Durable operation receipts considered for attribution, including incomplete ones.
	pub evidence: Vec<ChangeEvidence>,
	/// Receipts conflicted or exceeded the bound; attribution remains unknown.
	#[serde(default)]
	pub evidence_incomplete: bool,
	/// Plane where this change was captured.
	pub plane_id: PlaneId,
	/// Workspace identity, absent only for explicit Local-checkout Runs.
	pub workspace_id: Option<WorkspaceId>,
	/// Owning Conversation.
	pub conversation_id: ConversationId,
	/// Owning Run.
	pub run_id: RunId,
	/// Turn number within this Run.
	pub turn: u32,
	/// Completion or interruption.
	pub outcome: TurnOutcome,
	/// State before the turn.
	pub before: ChangeSnapshot,
	/// State after the turn.
	pub after: ChangeSnapshot,
	/// Changed paths with exact Git object and mode evidence.
	pub files: Vec<ChangedFile>,
	/// Complete binary-capable turn patch.
	pub artifact: ChangeArtifact,
}
/// A diff's boundaries, metadata, and bounded patch preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeDiff {
	/// Total number of changed paths across every page.
	pub total_files: u32,
	/// Opaque continuation bound to this query and snapshot; expires after five minutes.
	pub next_page: Option<crate::PageCursor>,
	/// Journal fence for the durable metadata; Current also inspects live Git.
	pub cursor: crate::EventSequence,
	/// Plane where these changes were observed.
	pub plane_id: PlaneId,
	/// Workspace, absent for Local-checkout Runs.
	pub workspace_id: Option<WorkspaceId>,
	/// Owning Run.
	pub run_id: RunId,
	/// Compared boundaries.
	pub scope: DiffScope,
	/// Number of retained completed or interrupted turns.
	pub latest_turn: u32,
	/// Present for a per-turn diff.
	pub outcome: Option<TurnOutcome>,
	/// Earlier state.
	pub before: ChangeSnapshot,
	/// Later state.
	pub after: ChangeSnapshot,
	/// Changed paths and origin evidence.
	pub files: Vec<ChangedFile>,
	/// Complete patch.
	pub artifact: ChangeArtifact,
	/// UTF-8 preview of the patch; binary files use Git binary patches.
	pub patch: String,
	/// Whether the Artifact contains more bytes than this preview.
	pub patch_truncated: bool,
}

/// A bounded slice of an immutable patch, for clients to verify and assemble.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeArtifactChunk {
	/// Complete content address and size.
	pub artifact: ChangeArtifact,
	/// Offset of these bytes in the complete Artifact.
	pub offset: u64,
	/// At most 64 KiB. Empty only at the end of the Artifact.
	pub bytes: Vec<u8>,
}
