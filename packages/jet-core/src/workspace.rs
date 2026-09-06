//! Managed Workspaces and where a Conversation does its work (ADR-0025).
//!
//! A managed Conversation works in a Workspace: a Git worktree of its
//! Project at a Jet-owned root, checked out detached at the commit the
//! user's selected base resolved to when the Workspace was made. Two
//! Conversations never share one, so they cannot overwrite each other. A
//! Conversation may instead work in the Project's own Local checkout,
//! explicitly and without isolation: Jet admits one live managed Run there
//! at a time and cannot lock the processes outside its management.
//!
//! A Workspace may start with changes from the Local checkout. They are
//! captured before the transaction opens and applied after the worktree
//! exists, and a Workspace they cannot be applied to is not kept (see
//! [`crate::seed`]).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jet_store::{
	NewConversation, NewWorkspace, RetentionPolicy, WorkingTreeRecord,
	WorkspaceRecord, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::command::CommandOutcome;
use crate::conversation::{Conversation, ConversationId, ConversationOrigin};
use crate::error::CoreError;
use crate::event::{EventKind, EventSubject};
use crate::filesystem::{blocking, canonicalize};
use crate::promotion::WorkspacePromotion;
use crate::seed::{SeedSelection, WorkspaceSeed};
use crate::seed_capture::{self, CapturedSeed};
use crate::{Actor, Core, ProjectId, lifecycle, system_time, worktree};

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
		/// Which Local-checkout changes to start with.
		seed: SeedSelection,
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
	/// The Local-checkout changes it started with, if any.
	pub seed: Option<WorkspaceSeed>,
	/// Its most recent promotion, if it has been promoted: where that
	/// stands, and the paths it could not settle when it is conflicted
	/// (ADR-0025).
	pub promotion: Option<WorkspacePromotion>,
	/// When it was created.
	pub created_at: SystemTime,
}

/// A Workspace request whose Project and base were looked up, and whose
/// seed was captured, before the transaction opened; ready to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedWorkspace {
	project_id: ProjectId,
	project_root: PathBuf,
	base: WorkspaceBase,
	captured: Option<CapturedSeed>,
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

/// Looks up the Project, resolves the base, and captures the selected
/// Local-checkout changes before the transaction opens, so the reads of
/// the repository happen outside the store's lock and a refusal that
/// describes the repository leaves no receipt behind (ADR-0093).
///
/// # Errors
///
/// Returns `workspace.base_invalid`, `project.not_found`,
/// `workspace.base_not_found`, what [`seed_capture::capture`] refuses, or
/// what Git reports when it cannot answer.
pub(crate) async fn prepare(
	core: &Core,
	project_id: ProjectId,
	base: &BaseSelection,
	seed: &SeedSelection,
) -> Result<PreparedWorkspace, CoreError> {
	base.validate()?;
	seed.validate()?;
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
	let captured = if seed.is_none() {
		None
	} else {
		Some(
			capture_in_scratch(
				&core.workspace_home,
				&project_root,
				seed,
				&commit,
			)
			.await?,
		)
	};
	Ok(PreparedWorkspace {
		project_id,
		project_root,
		base: WorkspaceBase {
			selection: base.clone(),
			commit,
		},
		captured,
	})
}

/// Captures `seed` through a scratch directory of the Workspace home.
async fn capture_in_scratch(
	home: &WorkspaceHome,
	project_root: &Path,
	seed: &SeedSelection,
	commit: &str,
) -> Result<CapturedSeed, CoreError> {
	with_scratch(home, "seed", async |scratch| {
		seed_capture::capture(project_root, scratch, seed, commit).await
	})
	.await
}

/// Runs `work` with a scratch directory of the Workspace home, which is
/// the owner's alone and is gone again before this returns.
pub(crate) async fn with_scratch<T>(
	home: &WorkspaceHome,
	purpose: &str,
	work: impl AsyncFnOnce(&Path) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
	use std::os::unix::fs::DirBuilderExt;
	let scratch = prepare_home(&home.0)
		.await?
		.join(format!(".{purpose}-{}", Uuid::now_v7()));
	let created = scratch.clone();
	blocking(move || std::fs::DirBuilder::new().mode(0o700).create(&created))
		.await?
		.map_err(home_unavailable)?;
	let result = work(&scratch).await;
	let _ = blocking(move || std::fs::remove_dir_all(&scratch)).await;
	result
}

/// Records a Conversation that works in a new Workspace and creates the
/// worktree, seeded when a seed was captured, in the transaction that
/// commits the rows.
///
/// The worktree is added last, after every row, so a refused row costs no
/// disk; and inside the transaction rather than before it, so a retried
/// Command that already committed creates nothing twice (ADR-0093). A
/// commit that fails after the worktree exists leaves a directory no row
/// names, which no later Conversation can collide with: the root is the
/// Conversation's own identity. A seed that cannot be applied fails the
/// whole creation and removes the worktree, so no Workspace is left
/// holding part of what was selected (ADR-0025).
///
/// The Project is read again here: what was prepared describes the
/// repository, and whether the Project is still registered is the
/// transaction's to answer.
///
/// # Errors
///
/// Returns `project.not_found` when the Project is no longer registered,
/// `workspace.seed_failed` when the seed cannot be applied, and what the
/// store or Git reports when the Workspace cannot be made.
pub(crate) async fn create(
	tx: &mut WriteTransaction,
	actor: &Actor,
	retention: RetentionPolicy,
	origin: ConversationOrigin,
	prepared: PreparedWorkspace,
	home: &WorkspaceHome,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let PreparedWorkspace {
		project_id,
		project_root,
		base,
		captured,
	} = prepared;
	let seed = captured.as_ref().map(WorkspaceSeed::from);
	if tx.project(project_id.0).await?.is_none() {
		return Err(project_not_found());
	}
	let conversation_id = Uuid::now_v7();
	let root = prepare_home(&home.0)
		.await?
		.join(conversation_id.to_string());
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
			origin: origin.record(),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let workspace: Workspace = tx
		.insert_workspace(NewWorkspace {
			workspace_id: Uuid::now_v7(),
			conversation_id,
			project_id: project_id.0,
			root: root_text.clone(),
			base_selection: base.selection.as_revision().to_owned(),
			base_commit: base.commit.clone(),
			seed: seed.as_ref().map(Into::into),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let subject = EventSubject::Conversation(conversation.conversation_id);
	let created = EventKind::ConversationCreated {
		retention,
		working_tree: conversation.working_tree,
		origin,
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
	if let Some(seed) = seed {
		let seeded = EventKind::WorkspaceSeeded {
			workspace_id: workspace.workspace_id,
			seed,
		};
		tx.append_event(seeded.to_record(actor, subject, now_unix_ms)?)
			.await?;
	}
	worktree::add_detached(&project_root, &root_text, &base.commit).await?;
	if let Some(captured) = captured
		&& let Err(refusal) = seed_capture::apply(&root, &captured).await
	{
		worktree::remove_forced(&project_root, &root_text).await;
		return Err(refusal);
	}
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
	origin: ConversationOrigin,
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
			origin: origin.record(),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationCreated {
		retention,
		working_tree: conversation.working_tree,
		origin,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(conversation.conversation_id),
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

/// Admits a new Run to a Project's Local checkout: one live managed Run
/// at a time (ADR-0025).
///
/// # Errors
///
/// Returns a `conflict` `run.local_checkout_busy` when another
/// Conversation's Run is live there. The message says what Jet cannot
/// do about the rest: processes outside its management are not locked.
pub(crate) async fn admit_local_checkout_run(
	tx: &mut WriteTransaction,
	project_id: ProjectId,
) -> Result<(), CoreError> {
	if lifecycle::any_live(&tx.local_checkout_runs(project_id.0).await?) {
		return Err(CoreError::conflict(
			"run.local_checkout_busy",
			"the Project's Local checkout already has a live managed Run; Jet \
			 admits one at a time there because it cannot lock the processes \
			 outside its management, so start this Conversation in a Workspace \
			 instead",
		));
	}
	Ok(())
}

/// Creates the Workspace home for the owner alone, if it is not there yet,
/// and returns it as the filesystem names it, so a Workspace root is
/// canonical however the Jet home was spelled.
async fn prepare_home(home: &Path) -> Result<PathBuf, CoreError> {
	use std::os::unix::fs::DirBuilderExt;
	let created = home.to_path_buf();
	blocking(move || {
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(&created)
	})
	.await?
	.map_err(home_unavailable)?;
	canonicalize(home.to_path_buf())
		.await
		.map_err(home_unavailable)
}

fn home_unavailable(error: std::io::Error) -> CoreError {
	CoreError::unavailable(
		"workspace.home_unavailable",
		"the Workspace home cannot be created on this Plane",
		error.to_string(),
	)
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
			seed: record.seed.map(Into::into),
			promotion: None,
			created_at: system_time(record.created_at_unix_ms),
		}
	}
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
