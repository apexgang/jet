//! Wire form of managed Workspaces and of where a Conversation does its
//! work (ADR-0025).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::promotion::WorkspacePromotion;

/// Where a Conversation does its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkingTree {
	/// In no Project. Nothing on disk belongs to the Conversation.
	NoProject,
	/// In a managed Workspace of a Project, isolated from every other
	/// Conversation. The Workspace itself comes with the Conversation
	/// snapshot.
	Workspace {
		/// The Project the Workspace was created from.
		project_id: Uuid,
	},
	/// In the Project's own Local checkout, without isolation. The Plane
	/// admits one live managed Run there at a time and cannot lock the
	/// processes outside its management.
	LocalCheckout {
		/// The Project whose checkout it works in.
		project_id: Uuid,
	},
}

/// What a new Conversation asks for as its working tree. Left out, it asks
/// for no Project, which is what every request before protocol minor 9
/// asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkingTreeRequest {
	/// No Project yet.
	#[default]
	NoProject,
	/// A managed Workspace of a Project: the default for managed work.
	Workspace {
		/// The Project to create the Workspace from.
		project_id: Uuid,
		/// The base to start from; the Project's HEAD when left out.
		#[serde(default)]
		base: BaseSelection,
		/// Which Local-checkout changes to start with; none when left out,
		/// which is what every request before protocol minor 10 asked for.
		#[serde(default, skip_serializing_if = "SeedSelection::is_none")]
		seed: SeedSelection,
	},
	/// The Project's own Local checkout, chosen explicitly.
	LocalCheckout {
		/// The Project whose checkout to work in.
		project_id: Uuid,
	},
}

impl WorkingTreeRequest {
	/// Whether the request is the one an older minor implies by saying
	/// nothing, so the field is left out for a Plane that does not know it.
	#[must_use]
	pub fn is_no_project(&self) -> bool {
		matches!(self, Self::NoProject)
	}

	/// Whether the request asks for Local-checkout changes, which needs
	/// protocol minor 10.
	#[must_use]
	pub fn is_seeded(&self) -> bool {
		matches!(self, Self::Workspace { seed, .. } if !seed.is_none())
	}
}

/// Which Local-checkout changes a new Workspace starts with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeedSelection {
	/// No changes: the Workspace starts at its base alone.
	#[default]
	None,
	/// Every eligible change: modifications and deletions of tracked paths,
	/// untracked paths that are not ignored, and the commit each submodule
	/// has checked out. Ignored paths and nested repositories are left out.
	AllEligible,
	/// The named paths and whatever they hold. An ignored path is included
	/// because it was named; a named directory brings its unignored content.
	Paths {
		/// Paths relative to the Project root, with `/` between components.
		paths: Vec<String>,
	},
}

impl SeedSelection {
	/// Whether the selection asks for nothing, so the field is left out.
	#[must_use]
	pub fn is_none(&self) -> bool {
		matches!(self, Self::None)
	}
}

/// What a Workspace was seeded with from its Project's Local checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSeed {
	/// The Git tree object the changes were captured as, as Git spells it.
	pub tree: String,
	/// How many paths that tree changes against the base commit.
	pub changed_paths: u32,
}

/// The base a Workspace starts from, as the user selects it. The Plane
/// resolves it to one commit when the Workspace is created and never
/// again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BaseSelection {
	/// Whatever the Project's Local checkout has checked out.
	#[default]
	Head,
	/// A branch, tag, or other revision as Git spells it.
	Revision {
		/// The revision name.
		revision: String,
	},
}

/// The immutable base of a Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBase {
	/// The base as the user selected it.
	pub selection: BaseSelection,
	/// The commit it resolved to when the Workspace was created, as Git
	/// spells it.
	pub commit: String,
}

/// One managed Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
	/// Durable identity.
	pub workspace_id: Uuid,
	/// The one Conversation that owns it.
	pub conversation_id: Uuid,
	/// The Project it was created from.
	pub project_id: Uuid,
	/// The absolute root of its worktree, under the Plane's Jet home.
	pub root: String,
	/// What it started from.
	pub base: WorkspaceBase,
	/// The Local-checkout changes it started with, if any. Absent before
	/// protocol minor 10, and when it was seeded with nothing.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub seed: Option<WorkspaceSeed>,
	/// Its most recent promotion, if it has been promoted: where that
	/// stands, and the paths it could not settle when it is conflicted.
	/// Absent before protocol minor 11, and when it was never promoted.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub promotion: Option<WorkspacePromotion>,
	/// When it was created, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
}
