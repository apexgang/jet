//! Wire form of Workspace promotion: previewing what applying a
//! Workspace's changes to a permanent checkout or branch of its Project
//! would do, confirming exactly that, and where a promotion stands
//! (ADR-0025).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a promotion applies a Workspace's changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromotionDestination {
	/// The Project's own Local checkout. The changes arrive staged in its
	/// index and written to its files, merged over whatever it holds.
	LocalCheckout,
	/// A branch of the Project that no working tree has checked out. The
	/// changes arrive as one commit on top of the branch.
	Branch {
		/// The branch name, as Git spells it without `refs/heads/`.
		name: String,
	},
}

/// What a preview showed and a promotion carries back: the Workspace and
/// destination as they stood, the result and the risk the user looked
/// at, and the client it was shown to. The Plane refuses a promotion when
/// any of it has changed since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionBinding {
	/// The Workspace being promoted.
	pub workspace_id: Uuid,
	/// Where its changes go.
	pub destination: PromotionDestination,
	/// The commit the Workspace started from, which the merge is against.
	pub base_commit: String,
	/// The Workspace's working tree, captured as one Git tree.
	pub workspace_tree: String,
	/// The commit the destination had checked out or pointed at.
	pub destination_commit: String,
	/// The destination's content as one Git tree: its working tree for the
	/// Local checkout, the tip's tree for a branch.
	pub destination_tree: String,
	/// The tree the three-way merge produced.
	pub result_tree: String,
	/// Whether the destination holds uncommitted changes of its own, which
	/// the promotion is merged over and never discards.
	pub destination_dirty: bool,
	/// The paths the promotion cannot settle. A preview with any is shown
	/// and never applied.
	pub conflicts: Vec<PromotionConflict>,
	/// The Client identity the preview was shown to.
	pub actor: Uuid,
}

/// What promoting a Workspace would do, shown before it is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionPreview {
	/// Newest Event sequence visible when the Workspace was read, carried
	/// as a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// What the promotion is bound to, risk included.
	pub binding: PromotionBinding,
	/// How many paths the promotion changes in the destination.
	pub changed_paths: u32,
	/// The changes, up to the first 4096 of them, in Git's order.
	pub changes: Vec<PromotedChange>,
}

/// One path a promotion changes in the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotedChange {
	/// The path, as Git spells it.
	pub path: String,
	/// What happens to it.
	pub kind: ChangeKind,
}

/// What a promotion does to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
	/// The destination gains the path.
	Added,
	/// The destination's content or mode at the path changes.
	Modified,
	/// The destination loses the path.
	Deleted,
}

/// One path a promotion cannot settle without a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionConflict {
	/// The path, as Git spells it.
	pub path: String,
	/// Why it cannot be settled.
	pub kind: ConflictKind,
}

/// Why a path cannot be promoted as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
	/// The Workspace and the destination both changed the path since the
	/// base, in ways Git cannot combine.
	Diverged,
	/// The Workspace adds the path and the destination already holds an
	/// ignored file there, which the merge never saw.
	Untracked,
	/// The destination's index holds a version of the path that differs
	/// from its file and from HEAD alike, which the merge never saw.
	Staged,
}

/// Where a promotion stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionState {
	/// Recorded, with the Effect that applies it not yet settled.
	Applying,
	/// Applied, and the destination verified to hold the result.
	Promoted,
	/// Never applied: the preview could not settle every path, and the
	/// paths are kept with the promotion for the user to resolve in the
	/// Workspace.
	Conflicted,
	/// Its Effect reported a definite failure before changing anything;
	/// the destination is as it was.
	Failed,
	/// Its Effect's outcome could not be established. The Plane neither
	/// repeats it nor calls it failed; the destination is the user's to
	/// inspect.
	OutcomeUnknown,
}

/// One recorded promotion of a Workspace: what the user confirmed and
/// where it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePromotion {
	/// Durable identity.
	pub promotion_id: Uuid,
	/// Exactly what the preview bound and the user confirmed.
	pub binding: PromotionBinding,
	/// How many paths the result changes in the destination.
	pub changed_paths: u32,
	/// Where the promotion stands.
	pub state: PromotionState,
	/// When it was recorded, in signed Unix milliseconds.
	pub recorded_at_unix_ms: i64,
	/// When it reached a settled state, if it has.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub settled_at_unix_ms: Option<i64>,
}
