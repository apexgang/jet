//! Removing a Project, as a Command: prepared against a fresh inspection
//! before the transaction opens, and carried out inside it (ADR-0011,
//! ADR-0102, ADR-0105).

use super::{
	ProjectDisposal, ProjectRemovalBinding, ProjectRemoved, RemovalObstacle,
	disposal, inspection,
};
use crate::{
	Actor, CommandOutcome, Core, CoreError, ProjectId,
	audit::{self, AuditDecision, AuditSubject, Decision},
	event::{EventKind, EventSubject},
	project::require_interactive,
};
use jet_store::{ProjectRecord, WriteTransaction};
use std::path::PathBuf;

/// A removal whose binding still matches the Project, and what it takes.
#[derive(Debug)]
pub(crate) struct PreparedRemoval {
	project: ProjectRecord,
	workspace_roots: Vec<PathBuf>,
}

/// Computes the preview again and checks that `binding` is what it shows,
/// that it was shown to `actor`, that nothing refuses the removal, and
/// that `typed_name` spells the directory's own name. Nothing is changed
/// (ADR-0093).
///
/// # Errors
///
/// Returns `project.removal_unbound` when the binding was shown to
/// another Client, `project.not_found` when the Project is gone,
/// `project.live_work`, `project.protected_root`, or
/// `project.contains_project` when something refuses the removal,
/// `project.removal_stale` when the Project has moved past the binding,
/// and `project.name_mismatch` when the typed name is not the
/// directory's.
pub(crate) async fn prepare(
	core: &Core,
	actor: &Actor,
	binding: &ProjectRemovalBinding,
	typed_name: &str,
) -> Result<PreparedRemoval, CoreError> {
	require_interactive(actor);
	if binding.actor != actor.client_id() {
		return Err(CoreError::invalid_input(
			"project.removal_unbound",
			"the preview was shown to another Client; ask for it again",
		));
	}
	let inspected =
		inspection::inspect(core, actor.client_id(), binding.project_id)
			.await?;
	if let Some(obstacle) = inspected.obstacles.first() {
		return Err(refusal(*obstacle));
	}
	if inspected.binding != *binding {
		return Err(CoreError::conflict(
			"project.removal_stale",
			"the Project has changed since the preview; review it again",
		));
	}
	let name = inspected.binding.root.file_name().and_then(|n| n.to_str());
	if name != Some(typed_name) {
		return Err(CoreError::invalid_input(
			"project.name_mismatch",
			"type the Project directory's own name to confirm its removal",
		));
	}
	Ok(PreparedRemoval {
		project: inspected.project,
		workspace_roots: inspected
			.workspaces
			.into_iter()
			.map(|workspace| PathBuf::from(workspace.root))
			.collect(),
	})
}

fn refusal(obstacle: RemovalObstacle) -> CoreError {
	match obstacle {
		RemovalObstacle::LiveRuns | RemovalObstacle::Schedules => {
			CoreError::conflict(
				"project.live_work",
				"the Project has live Runs or Scheduled tasks; stop and \
				 cancel them first",
			)
		}
		RemovalObstacle::FilesystemRoot
		| RemovalObstacle::UserHome
		| RemovalObstacle::JetHome => CoreError::invalid_input(
			"project.protected_root",
			"a filesystem root, the user's home, and Jet's home are never \
			 removed",
		),
		RemovalObstacle::ContainsProject => CoreError::conflict(
			"project.contains_project",
			"another registered Project lies inside this one; remove it \
			 first",
		),
	}
}

/// Removes the prepared Project: its rows, its identity in the Deletion
/// ledger, the journal Event, and the audit record, in the transaction;
/// then its directory, still inside it, so a directory the Plane cannot
/// dispose of leaves the registration in place and the Command refused
/// (ADR-0102, ADR-0105). The Plane's one writer waits while the
/// directory moves, which is a rename into the Trash and a walk for a
/// permanent removal. Its Workspace directories follow.
///
/// # Errors
///
/// Returns `project.live_work` when work was admitted since the
/// preparation, and what the disposal reports when it cannot finish.
pub(crate) async fn remove(
	tx: &mut WriteTransaction,
	actor: &Actor,
	prepared: PreparedRemoval,
	disposal: ProjectDisposal,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let PreparedRemoval {
		project,
		workspace_roots,
	} = prepared;
	let project_id = ProjectId(project.project_id);
	let work = tx.project_work(project.project_id).await?;
	if work.live_runs > 0 {
		return Err(refusal(RemovalObstacle::LiveRuns));
	}
	if work.schedules > 0 {
		return Err(refusal(RemovalObstacle::Schedules));
	}
	tx.delete_project(project.project_id, now).await?;
	let root = PathBuf::from(project.root);
	tx.append_event(
		EventKind::ProjectRemoved {
			project_id,
			root: root.clone(),
		}
		.to_record(actor, EventSubject::Plane, now)?,
	)
	.await?;
	// ASVS 16.2.1: the one Command that destroys a user's directory is
	// exactly what the Security audit exists to record (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::ProjectRemoved,
			AuditSubject::Project(project_id),
		),
		now,
	)
	.await?;
	let disposition = disposal::dispose(&root, disposal).await?;
	disposal::remove_workspaces(workspace_roots).await;
	Ok(CommandOutcome::ProjectRemoved(ProjectRemoved {
		project_id,
		root,
		disposition,
	}))
}

#[cfg(test)]
mod tests {
	use super::super::{
		Disposition, PERMANENT_REMOVAL_WARNING, PermanentRemoval,
		ProjectDisposal, ProjectRemovalBinding, ProjectRemovalPreview,
		ProjectRemoved, RemovalObstacle,
	};
	use crate::{
		AuditActor, AuditRisk, AuditSequence, ClientId, Command,
		CommandOutcome, Core, CoreError, EventKind, PathGrant, ProjectId,
		Query, QueryResult, WorkingTree,
		test_support::{
			actor, conversation_snapshot, events, init_repository,
			register_repository, request, start_core,
		},
		workspace::{BaseSelection, WorkingTreeRequest, seed::SeedSelection},
	};
	use jet_store::RetentionPolicy;
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	async fn preview(
		core: &Core,
		project_id: ProjectId,
	) -> ProjectRemovalPreview {
		let QueryResult::ProjectRemovalPreview(preview) = core
			.query(&actor(), Query::PreviewProjectRemoval { project_id })
			.await
			.unwrap()
		else {
			panic!("expected a removal preview");
		};
		preview
	}

	/// A store under its own `.jet`, so Jet's home is that directory and
	/// the repositories beside it are not inside it.
	fn store(dir: &tempfile::TempDir) -> std::path::PathBuf {
		let home = dir.path().join(".jet");
		std::fs::create_dir_all(&home).unwrap();
		home.join("plane.sqlite3")
	}

	fn permanent() -> ProjectDisposal {
		ProjectDisposal::Permanent(
			PermanentRemoval::acknowledging(PERMANENT_REMOVAL_WARNING).unwrap(),
		)
	}

	async fn remove(
		core: &Core,
		binding: ProjectRemovalBinding,
		typed_name: &str,
		disposal: ProjectDisposal,
	) -> Result<CommandOutcome, CoreError> {
		core.execute(
			&actor(),
			request(Command::RemoveProject {
				binding,
				typed_name: typed_name.into(),
				disposal,
			}),
		)
		.await
	}

	async fn audit(
		core: &Core,
	) -> Vec<(String, AuditActor, AuditRisk, String)> {
		let QueryResult::SecurityAudit(page) = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: AuditSequence(0),
				},
			)
			.await
			.unwrap()
		else {
			panic!("expected an audit page");
		};
		page.entries
			.into_iter()
			.map(|entry| {
				(entry.decision, entry.actor, entry.risk, entry.target.kind)
			})
			.collect()
	}

	/// The preview discloses what the removal loses and binds it; the
	/// removal takes the registration, the Workspaces, and the directory,
	/// detaches the Conversation, and reaches the journal, the audit, and
	/// the Deletion ledger (ADR-0011, ADR-0102, ADR-0105).
	#[tokio::test]
	async fn a_bound_removal_takes_the_project_and_its_workspaces() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&store(&dir)).await;
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
		let root = dir.path().join("repo").canonicalize().unwrap();
		std::fs::write(root.join("scratch.txt"), "work in progress\n").unwrap();
		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::Workspace {
						project_id,
						base: BaseSelection::Head,
						seed: SeedSelection::None,
					},
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Conversation");
		};
		let conversation_id = conversation.conversation_id;
		let workspace = conversation_snapshot(&core, conversation_id)
			.await
			.workspace
			.unwrap();

		let previewed = preview(&core, project_id).await;
		let removed =
			remove(&core, previewed.binding.clone(), "repo", permanent())
				.await
				.unwrap();
		let gone = core
			.query(&actor(), Query::PreviewProjectRemoval { project_id })
			.await
			.unwrap_err();
		let detached = conversation_snapshot(&core, conversation_id).await;
		let jet_store::DeletionLedger::Verified(ledger) =
			core.store.deletion_ledger().unwrap()
		else {
			panic!("ledger is corrupt");
		};

		let expected = ProjectRemovalBinding {
			project_id,
			root: root.clone(),
			live_runs: 0,
			schedules: 0,
			dirty_files: 1,
			unpushed_commits: 1,
			workspaces: vec![workspace.workspace_id],
			actor: ClientId(Uuid::nil()),
		};
		assert_eq!(
			(
				&previewed.binding,
				previewed.obstacles,
				previewed.disk_use_bytes > 0,
				removed,
				gone.code,
				root.exists(),
				workspace.root.exists(),
				(detached.conversation.working_tree, detached.workspace),
				events(&core)
					.await
					.into_iter()
					.filter(|kind| matches!(
						kind,
						EventKind::ProjectRemoved { .. }
					))
					.collect::<Vec<_>>(),
				audit(&core).await.pop(),
				ledger
					.iter()
					.map(|record| (record.kind, record.identity))
					.collect::<Vec<_>>(),
			),
			(
				&expected,
				vec![],
				true,
				CommandOutcome::ProjectRemoved(ProjectRemoved {
					project_id,
					root: root.clone(),
					disposition: Disposition::Deleted,
				}),
				"project.not_found".to_string(),
				false,
				false,
				(WorkingTree::NoProject, None),
				vec![EventKind::ProjectRemoved {
					project_id,
					root: root.clone(),
				}],
				Some((
					"project.removed".into(),
					AuditActor::InteractiveClient {
						client_id: ClientId(Uuid::nil()),
					},
					AuditRisk::Destructive,
					"project".into(),
				)),
				vec![(jet_store::DeletedIdentityKind::Project, project_id.0)],
			)
		);
	}

	/// A removal is refused, with nothing removed, when its binding was
	/// shown to another Client, when the Project moved past it, when the
	/// typed name is not the directory's, when work is live in it, when
	/// another Project lies inside it, and when the acknowledgement is not
	/// the warning (ADR-0011).
	#[tokio::test]
	async fn refusals_leave_the_project_in_place() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&store(&dir)).await;
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
		let root = dir.path().join("repo").canonicalize().unwrap();
		let binding = preview(&core, project_id).await.binding;

		let unbound = remove(
			&core,
			ProjectRemovalBinding {
				actor: ClientId(Uuid::new_v4()),
				..binding.clone()
			},
			"repo",
			permanent(),
		)
		.await
		.unwrap_err();
		let misnamed = remove(&core, binding.clone(), "Repo", permanent())
			.await
			.unwrap_err();
		std::fs::write(root.join("scratch.txt"), "moved on\n").unwrap();
		let stale = remove(&core, binding.clone(), "repo", permanent())
			.await
			.unwrap_err();
		let fresh = preview(&core, project_id).await.binding;
		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::LocalCheckout {
						project_id,
					},
				}),
			)
			.await
			.unwrap()
		else {
			panic!("expected a Conversation");
		};
		core.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation.conversation_id,
			}),
		)
		.await
		.unwrap();
		let live = remove(&core, fresh, "repo", permanent()).await.unwrap_err();
		let obstacles = preview(&core, project_id).await.obstacles;
		let nested = init_repository(&root.join("nested"));
		core.execute(
			&actor(),
			request(Command::RegisterProject {
				grant: PathGrant(nested),
			}),
		)
		.await
		.unwrap();
		let containing = preview(&core, project_id).await.obstacles;
		let unacknowledged =
			PermanentRemoval::acknowledging("I authorize this").unwrap_err();

		assert_eq!(
			(
				unbound.code,
				misnamed.code,
				stale.code,
				live.code,
				obstacles,
				containing,
				unacknowledged.code,
				root.is_dir(),
			),
			(
				"project.removal_unbound".to_string(),
				"project.name_mismatch".to_string(),
				"project.removal_stale".to_string(),
				"project.live_work".to_string(),
				vec![RemovalObstacle::LiveRuns],
				vec![
					RemovalObstacle::LiveRuns,
					RemovalObstacle::ContainsProject
				],
				"project.warning_unacknowledged".to_string(),
				true,
			)
		);
	}

	/// A Project cannot be removed by an Actor the core does not know as
	/// interactive: both Actors it knows are, and the match is exhaustive,
	/// so this is settled at compile time (ADR-0101). What is tested here
	/// is the other half: a Project lying inside Jet's home is refused.
	#[tokio::test]
	async fn a_project_inside_jet_home_is_refused() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&store(&dir)).await;
		let project_id =
			register_repository(&core, &dir.path().join(".jet/inside")).await;
		let binding = preview(&core, project_id).await.binding;

		let refused = remove(&core, binding, "inside", permanent())
			.await
			.unwrap_err();

		assert_eq!(
			(refused.code, preview(&core, project_id).await.obstacles),
			(
				"project.protected_root".to_string(),
				vec![RemovalObstacle::JetHome]
			)
		);
	}

	/// The directory goes to the system Trash, from where it can come
	/// back. The Trash used is the user's own, so the entry is purged
	/// again once it has been seen there.
	#[cfg(target_os = "linux")]
	#[tokio::test]
	async fn the_directory_goes_to_the_system_trash() {
		let dir = tempfile::tempdir().unwrap();
		let core = start_core(&store(&dir)).await;
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
		let root = dir.path().join("repo").canonicalize().unwrap();
		let binding = preview(&core, project_id).await.binding;

		let removed =
			remove(&core, binding, "repo", ProjectDisposal::SystemTrash).await;
		let trashed = trash::os_limited::list()
			.unwrap()
			.into_iter()
			.filter(|item| item.original_path() == root)
			.collect::<Vec<_>>();
		let listed = trashed.len();
		trash::os_limited::purge_all(trashed).unwrap();

		assert_eq!(
			(
				removed.map(|outcome| matches!(
					outcome,
					CommandOutcome::ProjectRemoved(ProjectRemoved {
						disposition: Disposition::Trashed,
						..
					})
				)),
				listed,
				root.exists()
			),
			(Ok(true), 1, false)
		);
	}
}
