//! What removing a Project would meet: read from the store, the
//! filesystem, and Git, before the decision is made (ADR-0011).

use super::{ProjectRemovalBinding, ProjectRemovalPreview, RemovalObstacle};
use crate::{
	Actor, ClientId, Core, CoreError, EventSequence, ProjectId, QueryResult,
	WorkspaceId,
	filesystem::blocking,
	project::{repository::git, require_interactive},
};
use jet_store::{ProjectRecord, ProjectWork, WorkspaceRecord};
use std::{
	path::{Path, PathBuf},
	time::Duration,
};

/// How long one Git inspection may take. A stalled Git stalls one
/// decision, not the Plane.
const INSPECTION_BUDGET: Duration = Duration::from_secs(30);

/// The Project and what the removal would meet, as the Plane reads it
/// right now.
pub(super) struct Inspected {
	pub(super) project: ProjectRecord,
	pub(super) workspaces: Vec<WorkspaceRecord>,
	pub(super) binding: ProjectRemovalBinding,
	pub(super) disk_use_bytes: u64,
	pub(super) obstacles: Vec<RemovalObstacle>,
	pub(super) cursor: EventSequence,
}

/// Shows what removing `project_id` would meet, without changing anything.
///
/// # Errors
///
/// Returns `project.not_found`, or what the inspection reports when it
/// cannot answer.
pub(crate) async fn preview(
	core: &Core,
	actor: &Actor,
	project_id: ProjectId,
) -> Result<QueryResult, CoreError> {
	require_interactive(actor);
	let inspected = inspect(core, actor.client_id(), project_id).await?;
	Ok(QueryResult::ProjectRemovalPreview(ProjectRemovalPreview {
		cursor: inspected.cursor,
		binding: inspected.binding,
		disk_use_bytes: inspected.disk_use_bytes,
		obstacles: inspected.obstacles,
		permanent_removal_warning: super::PERMANENT_REMOVAL_WARNING,
	}))
}

/// Reads the Project, inspects its directory between two reads, outside
/// any transaction because that takes Git, and reads the work that would
/// refuse the removal last, so the binding is as fresh as it can be.
pub(super) async fn inspect(
	core: &Core,
	actor: ClientId,
	project_id: ProjectId,
) -> Result<Inspected, CoreError> {
	let (project, others, workspaces) = core
		.store
		.read(async |tx| {
			let Some(project) = tx.project(project_id.0).await? else {
				return Err(CoreError::not_found(
					"project.not_found",
					"the Project is not registered on this Plane",
				));
			};
			let others = tx
				.projects()
				.await?
				.into_iter()
				.filter(|other| other.project_id != project.project_id)
				.map(|other| PathBuf::from(other.root))
				.collect();
			Ok((
				project,
				others,
				tx.workspaces_of_project(project_id.0).await?,
			))
		})
		.await?;
	let root = PathBuf::from(&project.root);
	let core_home = core.jet_home();
	let uncommitted = Uncommitted::inspect(&root).await?;
	let disk_use_bytes = {
		let root = root.clone();
		blocking(move || disk_use(&root)).await?
	};
	let (work, cursor) = core
		.store
		.read(async |tx| {
			Ok::<_, CoreError>((
				tx.project_work(project_id.0).await?,
				EventSequence(tx.event_cursor().await?),
			))
		})
		.await?;
	let roots = blocking(move || Roots {
		jet_home: canonical_or_as_named(core_home),
		user_home: std::env::var_os("HOME")
			.map(PathBuf::from)
			.map(canonical_or_as_named),
		others,
	})
	.await?;
	let obstacles = obstacles(&root, &roots, work);
	Ok(Inspected {
		binding: ProjectRemovalBinding {
			project_id,
			root,
			live_runs: work.live_runs,
			schedules: work.schedules,
			dirty_files: uncommitted.dirty_files,
			unpushed_commits: uncommitted.unpushed_commits,
			workspaces: workspaces
				.iter()
				.map(|workspace| WorkspaceId(workspace.workspace_id))
				.collect(),
			actor,
		},
		project,
		workspaces,
		disk_use_bytes,
		obstacles,
		cursor,
	})
}

/// The directories a Project may not be, hold, or lie in.
pub(super) struct Roots {
	/// Jet's own home, which holds the store and every Workspace.
	pub(super) jet_home: PathBuf,
	/// The user's home, when the environment names one.
	pub(super) user_home: Option<PathBuf>,
	/// The roots of every other registered Project.
	pub(super) others: Vec<PathBuf>,
}

/// The directory as the filesystem resolves it, so it compares with a
/// registered root, which is canonical (ADR-0101); or as it was named,
/// when it cannot be resolved, which no registered root can then equal.
fn canonical_or_as_named(path: PathBuf) -> PathBuf {
	std::fs::canonicalize(&path).unwrap_or(path)
}

/// What refuses removing the Project at `root`, in a fixed order.
pub(super) fn obstacles(
	root: &Path,
	roots: &Roots,
	work: ProjectWork,
) -> Vec<RemovalObstacle> {
	let mut found = Vec::new();
	if work.live_runs > 0 {
		found.push(RemovalObstacle::LiveRuns);
	}
	if work.schedules > 0 {
		found.push(RemovalObstacle::Schedules);
	}
	if root.parent().is_none() {
		found.push(RemovalObstacle::FilesystemRoot);
	}
	if roots.user_home.as_deref() == Some(root) {
		found.push(RemovalObstacle::UserHome);
	}
	if roots.jet_home.starts_with(root) || root.starts_with(&roots.jet_home) {
		found.push(RemovalObstacle::JetHome);
	}
	if roots
		.others
		.iter()
		.any(|other| other != root && other.starts_with(root))
	{
		found.push(RemovalObstacle::ContainsProject);
	}
	found
}

/// What the checkout at the root holds that the removal would lose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Uncommitted {
	dirty_files: u64,
	unpushed_commits: u64,
}

impl Uncommitted {
	/// Counts the lines of `git status`, one per changed file or untracked
	/// entry, and the commits no remote branch holds. A root that is gone
	/// holds nothing to lose.
	async fn inspect(root: &Path) -> Result<Self, CoreError> {
		if !root.is_dir() {
			return Ok(Self::default());
		}
		let status = run(
			root,
			&["status", "--porcelain=v1", "--untracked-files=normal"],
		)
		.await?;
		let unpushed = run(
			root,
			&["rev-list", "--count", "--all", "--not", "--remotes"],
		)
		.await?;
		Ok(Self {
			dirty_files: status.lines().count() as u64,
			unpushed_commits: unpushed.trim().parse().map_err(|_| {
				unreadable(format!("git counted {unpushed:?} commits"))
			})?,
		})
	}
}

async fn run(root: &Path, arguments: &[&str]) -> Result<String, CoreError> {
	let output = tokio::time::timeout(INSPECTION_BUDGET, git(root, arguments))
		.await
		.map_err(|_| unreadable("git did not finish in time".into()))??;
	if !output.status.success() {
		return Err(unreadable(output.stderr));
	}
	Ok(output.stdout)
}

fn unreadable(detail: String) -> CoreError {
	CoreError::unavailable(
		"project.unreadable",
		"the Project's checkout could not be inspected",
		detail.chars().take(512).collect::<String>(),
	)
}

/// Bytes the files under `root` occupy. Symbolic links count as
/// themselves and are not followed, so a link out of the root adds
/// nothing that the removal would not take.
fn disk_use(root: &Path) -> u64 {
	let mut total = 0;
	let mut pending = vec![root.to_path_buf()];
	while let Some(dir) = pending.pop() {
		let Ok(entries) = std::fs::read_dir(&dir) else {
			continue;
		};
		for entry in entries.flatten() {
			let Ok(metadata) = entry.metadata() else {
				continue;
			};
			if metadata.is_dir() {
				pending.push(entry.path());
			} else {
				total += metadata.len();
			}
		}
	}
	total
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	/// The roots a removal never takes are answered as obstacles, each
	/// for its own reason, and a root that is none of them has none
	/// (ADR-0011).
	#[test]
	fn protected_roots_are_obstacles() {
		let roots = Roots {
			jet_home: PathBuf::from("/home/u/.jet"),
			user_home: Some(PathBuf::from("/home/u")),
			others: vec![
				PathBuf::from("/home/u/src/parent/child"),
				PathBuf::from("/home/u/src/other"),
			],
		};
		let idle = ProjectWork {
			live_runs: 0,
			schedules: 0,
		};
		let busy = ProjectWork {
			live_runs: 2,
			schedules: 1,
		};
		let at = |root: &str, work| obstacles(Path::new(root), &roots, work);

		assert_eq!(
			(
				at("/", idle),
				at("/home/u", idle),
				at("/home/u/.jet", idle),
				at("/home/u/.jet/workspaces/w", idle),
				at("/home/u/src/parent", idle),
				at("/home/u/src/other", busy),
				at("/home/u/src/plain", idle),
			),
			(
				vec![
					RemovalObstacle::FilesystemRoot,
					RemovalObstacle::JetHome,
					RemovalObstacle::ContainsProject,
				],
				vec![
					RemovalObstacle::UserHome,
					RemovalObstacle::JetHome,
					RemovalObstacle::ContainsProject,
				],
				vec![RemovalObstacle::JetHome],
				vec![RemovalObstacle::JetHome],
				vec![RemovalObstacle::ContainsProject],
				vec![RemovalObstacle::LiveRuns, RemovalObstacle::Schedules],
				vec![],
			)
		);
	}

	/// Disk use counts every file under the root once and follows no
	/// symbolic link out of it.
	#[test]
	fn disk_use_counts_files_and_follows_no_links() {
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path().join("root");
		std::fs::create_dir_all(root.join("nested")).unwrap();
		std::fs::write(root.join("a"), [0; 100]).unwrap();
		std::fs::write(root.join("nested/b"), [0; 50]).unwrap();
		std::fs::write(dir.path().join("outside"), [0; 1000]).unwrap();
		std::os::unix::fs::symlink(dir.path().join("outside"), root.join("l"))
			.unwrap();

		let link = std::fs::symlink_metadata(root.join("l")).unwrap().len();

		assert_eq!(disk_use(&root), 150 + link);
	}
}
