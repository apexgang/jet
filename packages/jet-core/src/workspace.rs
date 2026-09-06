//! Managed Workspaces and where a Conversation does its work (ADR-0025).
//!
//! A managed Conversation works in a Workspace: a Git worktree of its
//! Project at a Jet-owned root, checked out detached at the commit the
//! user's selected base resolved to when the Workspace was made. Two
//! Conversations never share one, so they cannot overwrite each other. A
//! Conversation may instead work in the Project's own Local checkout,
//! explicitly and without isolation: Jet admits one live managed Run there
//! at a time and cannot lock the processes outside its management.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jet_store::{
	NewConversation, NewWorkspace, RetentionPolicy, WorkingTreeRecord,
	WorkspaceRecord, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::command::CommandOutcome;
use crate::conversation::{Conversation, ConversationId};
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::filesystem::blocking;
use crate::{Actor, Core, ProjectId, system_time, worktree};

/// Longest base selection the core accepts, as text. A branch name is
/// far shorter; the bound keeps a hostile client from storing a novel.
const MAX_SELECTION_CHARS: usize = 1024;

/// Durable identity of one Workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub Uuid);

/// The directory under which this core creates Workspaces, one per
/// Conversation, owned by Jet and by nothing else (ADR-0014, ADR-0025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceHome(pub PathBuf);

/// Where a Conversation does its work (ADR-0025).
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkingTree {
	/// In no Project. Nothing on disk belongs to the Conversation, and it
	/// has no Run until it is given somewhere to work.
	#[default]
	NoProject,
	/// In a managed Workspace of a Project, isolated from every other
	/// Conversation. The Workspace itself is read with the Conversation.
	Workspace {
		/// The Project the Workspace was created from.
		project_id: ProjectId,
	},
	/// In the Project's own Local checkout, alongside whatever else works
	/// there. Jet admits one live managed Run in a Local checkout at a time
	/// and cannot lock the processes outside its management.
	LocalCheckout {
		/// The Project whose checkout it works in.
		project_id: ProjectId,
	},
}

/// What a new Conversation asks for as its working tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum WorkingTreeRequest {
	/// No Project yet.
	NoProject,
	/// A managed Workspace of a Project, the default for managed work.
	Workspace {
		/// The Project to create the Workspace from.
		project_id: ProjectId,
		/// The base to start from.
		base: BaseSelection,
	},
	/// The Project's own Local checkout, chosen explicitly.
	LocalCheckout {
		/// The Project whose checkout to work in.
		project_id: ProjectId,
	},
}

/// The base a Workspace starts from, as the user selects it. It is
/// resolved to one commit when the Workspace is created and never again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseSelection {
	/// Whatever the Project's Local checkout has checked out.
	Head,
	/// A branch, tag, or other revision as Git spells it.
	Revision(String),
}

/// The immutable base of a Workspace: what was selected, and the commit it
/// resolved to when the Workspace was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBase {
	/// The base as the user selected it.
	pub selection: BaseSelection,
	/// The commit that selection named, as Git spells it.
	pub commit: String,
}

/// One managed Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
	/// Durable identity.
	pub workspace_id: WorkspaceId,
	/// The one Conversation that owns it.
	pub conversation_id: ConversationId,
	/// The Project it was created from.
	pub project_id: ProjectId,
	/// The canonical absolute root of its worktree, under the Workspace
	/// home.
	pub root: PathBuf,
	/// What it started from.
	pub base: WorkspaceBase,
	/// When it was created.
	pub created_at: SystemTime,
}

/// A Workspace request whose Project and base were looked up before the
/// transaction opened, ready to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceSeed {
	project_id: ProjectId,
	project_root: PathBuf,
	base: WorkspaceBase,
}

impl BaseSelection {
	/// The revision Git resolves, and the spelling the store keeps.
	fn as_revision(&self) -> &str {
		match self {
			Self::Head => "HEAD",
			Self::Revision(revision) => revision,
		}
	}

	fn from_stored(selection: String) -> Self {
		if selection == "HEAD" {
			Self::Head
		} else {
			Self::Revision(selection)
		}
	}

	/// Refuses a selection Git could read as something other than one
	/// revision name, before it reaches a subprocess or a row.
	fn validate(&self) -> Result<(), CoreError> {
		let revision = self.as_revision();
		let malformed = revision.is_empty()
			|| revision.chars().count() > MAX_SELECTION_CHARS
			|| revision.chars().any(char::is_control);
		if malformed {
			return Err(CoreError::invalid_input(
				"workspace.base_invalid",
				"a base selection is one revision name without control \
				 characters",
			));
		}
		Ok(())
	}
}

/// Looks up the Project and resolves the base before the transaction
/// opens, so the read of the repository happens outside the store's lock
/// and a refusal that describes the repository leaves no receipt behind
/// (ADR-0093).
///
/// # Errors
///
/// Returns `workspace.base_invalid`, `project.not_found`,
/// `workspace.base_not_found`, or what Git reports when it cannot answer.
pub(crate) async fn prepare(
	core: &Core,
	project_id: ProjectId,
	base: &BaseSelection,
) -> Result<WorkspaceSeed, CoreError> {
	base.validate()?;
	let project = core
		.store
		.read(async |tx| Ok::<_, CoreError>(tx.project(project_id.0).await?))
		.await?;
	let Some(project) = project else {
		return Err(project_not_found());
	};
	let project_root = PathBuf::from(project.root);
	let commit =
		worktree::resolve_commit(&project_root, base.as_revision()).await?;
	Ok(WorkspaceSeed {
		project_id,
		project_root,
		base: WorkspaceBase {
			selection: base.clone(),
			commit,
		},
	})
}

/// Records a Conversation that works in a new Workspace and creates the
/// worktree, in the transaction that commits the rows.
///
/// The worktree is added last, after every row, so a refused row costs no
/// disk; and inside the transaction rather than before it, so a retried
/// Command that already committed creates nothing twice (ADR-0093). A
/// commit that fails after the worktree exists leaves a directory no row
/// names, which no later Conversation can collide with: the root is the
/// Conversation's own identity.
///
/// # Errors
///
/// Returns what the store or Git reports when the Workspace cannot be
/// made.
pub(crate) async fn create(
	tx: &mut WriteTransaction,
	actor: &Actor,
	retention: RetentionPolicy,
	seed: WorkspaceSeed,
	home: &WorkspaceHome,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let WorkspaceSeed {
		project_id,
		project_root,
		base,
	} = seed;
	let conversation_id = Uuid::now_v7();
	let root = home.0.join(conversation_id.to_string());
	let Some(root_text) = root.to_str().map(str::to_owned) else {
		return Err(CoreError::internal(
			"workspace.root_not_unicode",
			"a Workspace root was not Unicode",
		));
	};
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id,
			retention,
			working_tree: WorkingTreeRecord::Workspace {
				project_id: project_id.0,
			},
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let workspace: Workspace = tx
		.insert_workspace(NewWorkspace {
			workspace_id: Uuid::now_v7(),
			conversation_id,
			project_id: project_id.0,
			root: root_text,
			base_selection: base.selection.as_revision().to_owned(),
			base_commit: base.commit.clone(),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let subject = EventSubject::Conversation(conversation.conversation_id);
	let created = EventKind::ConversationCreated {
		retention,
		working_tree: conversation.working_tree,
	};
	tx.append_event(created.to_record(actor, subject, now_unix_ms)?)
		.await?;
	let placed = EventKind::WorkspaceCreated {
		workspace_id: workspace.workspace_id,
		project_id,
		root: workspace.root.clone(),
		base: workspace.base.clone(),
	};
	tx.append_event(placed.to_record(actor, subject, now_unix_ms)?)
		.await?;
	prepare_home(&home.0).await?;
	worktree::add_detached(&project_root, &workspace.root, &base.commit)
		.await?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

/// Records a Conversation that works in a Project's Local checkout.
///
/// # Errors
///
/// Returns `project.not_found` when the Project is not registered.
pub(crate) async fn create_in_local_checkout(
	tx: &mut WriteTransaction,
	actor: &Actor,
	retention: RetentionPolicy,
	project_id: ProjectId,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	if tx.project(project_id.0).await?.is_none() {
		return Err(project_not_found());
	}
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
			working_tree: WorkingTreeRecord::LocalCheckout {
				project_id: project_id.0,
			},
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationCreated {
		retention,
		working_tree: conversation.working_tree,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(conversation.conversation_id),
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

/// Creates the Workspace home for the owner alone, if it is not there yet.
async fn prepare_home(home: &Path) -> Result<(), CoreError> {
	use std::os::unix::fs::DirBuilderExt;
	let home = home.to_path_buf();
	blocking(move || {
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(&home)
	})
	.await?
	.map_err(|error| {
		CoreError::unavailable(
			"workspace.home_unavailable",
			"the Workspace home cannot be created on this Plane",
			error.to_string(),
		)
	})
}

fn project_not_found() -> CoreError {
	CoreError::not_found("project.not_found", "the Project is not registered")
}

impl From<WorkingTreeRecord> for WorkingTree {
	fn from(record: WorkingTreeRecord) -> Self {
		match record {
			WorkingTreeRecord::NoProject => Self::NoProject,
			WorkingTreeRecord::Workspace { project_id } => Self::Workspace {
				project_id: ProjectId(project_id),
			},
			WorkingTreeRecord::LocalCheckout { project_id } => {
				Self::LocalCheckout {
					project_id: ProjectId(project_id),
				}
			}
		}
	}
}

impl From<WorkspaceRecord> for Workspace {
	fn from(record: WorkspaceRecord) -> Self {
		Self {
			workspace_id: WorkspaceId(record.workspace_id),
			conversation_id: ConversationId(record.conversation_id),
			project_id: ProjectId(record.project_id),
			root: PathBuf::from(record.root),
			base: WorkspaceBase {
				selection: BaseSelection::from_stored(record.base_selection),
				commit: record.base_commit,
			},
			created_at: system_time(record.created_at_unix_ms),
		}
	}
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
