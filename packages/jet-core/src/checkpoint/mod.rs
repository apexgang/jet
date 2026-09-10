//! Immutable turn boundaries and evidence-backed diffs (ADR-0030, ADR-0084).
pub(crate) mod capture;
pub(crate) mod change_artifact;
pub(crate) mod change_artifact_budget;
pub(crate) mod change_evidence;
pub(crate) mod omissions;
pub(crate) mod pages;
pub(crate) mod pressure;
pub(crate) mod query;
pub(crate) mod state;

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
	/// Payload capture was paused to preserve the disk-space reserve.
	DiskPressure,
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

#[cfg(test)]
pub(crate) mod tests {
	use std::{path::Path, sync::Arc};

	use pretty_assertions::assert_eq;
	use tokio::sync::{Mutex, mpsc};

	use crate::test_support::{actor, git, register_repository, request};
	use crate::*;

	#[tokio::test]
	async fn completed_turn_preserves_its_diff_after_later_edits_and_restart() {
		let dir = tempfile::tempdir().unwrap();
		let (core, sender) = start(dir.path()).await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
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
			panic!("Conversation")
		};
		let head = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "Edit".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		std::fs::write(root.join("README.md"), "Turn one\n").unwrap();
		sender
			.send(RunObservation::Completed("native-1".into()))
			.await
			.unwrap();
		sender.send(RunObservation::Ended(Some(0))).await.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Completed).await;
		let query = Query::ChangeDiff {
			run_id: run.run_id,
			scope: DiffScope::Turn { turn: 1 },
		};
		let original = core.query(&actor(), query.clone()).await.unwrap();
		let QueryResult::ChangeDiff(diff) = &original else {
			panic!("Diff")
		};
		assert_eq!((&diff.before.commit, &diff.after.commit), (&head, &head));
		assert!(
			diff.patch.contains("-# Jet\n+Turn one\n"),
			"checkpoint diff must contain the first turn's edit"
		);
		assert_eq!(
			diff.files
				.iter()
				.map(|file| (&*file.path, &file.origin))
				.collect::<Vec<_>>(),
			vec![("README.md", &ChangeOrigin::ExternalOrUnknown)]
		);
		assert_eq!(git(&root, &["rev-parse", "HEAD"]).trim(), head);
		std::fs::write(root.join("README.md"), "Later edit\n").unwrap();
		core.close().await;
		let (core, _) = start(dir.path()).await;
		assert_eq!(core.query(&actor(), query).await.unwrap(), original);
	}

	#[tokio::test]
	async fn interrupted_turn_and_history_survive_rewritten_commits() {
		let dir = tempfile::tempdir().unwrap();
		let (core, sender) = start(dir.path()).await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
		let original_head =
			git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
		std::fs::write(root.join("README.md"), "Initial dirty content\n")
			.unwrap();
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "Edit".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		std::fs::write(root.join("README.md"), "Committed turn\n").unwrap();
		git(&root, &["add", "-A"]);
		git(&root, &["commit", "-qm", "Harness commit"]);
		sender
			.send(RunObservation::TurnEnded(TurnOutcome::Completed))
			.await
			.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		let first =
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
		assert!(
			first
				.patch
				.contains("-Initial dirty content\n+Committed turn\n")
		);
		sender.send(RunObservation::TurnStarted).await.unwrap();
		sender
			.send(RunObservation::Activity(RunActivity::WaitingForUser))
			.await
			.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 3,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				if let QueryResult::RunExecution(state) = core
					.query(&actor(), Query::RunExecution { run_id: run.run_id })
					.await
					.unwrap() && state.activity
					== Some(RunActivity::WaitingForUser)
				{
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		git(&root, &["reset", "--hard", &original_head]);
		std::fs::write(root.join("README.md"), "Partial second turn\n")
			.unwrap();
		sender.send(RunObservation::Ended(None)).await.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 4,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Failed).await;
		let second =
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 2 }).await;
		assert_eq!(second.outcome, Some(TurnOutcome::Interrupted));
		assert!(
			second
				.patch
				.contains("-Committed turn\n+Partial second turn\n")
		);
		git(&root, &["reflog", "expire", "--expire=now", "--all"]);
		git(&root, &["gc", "--prune=now"]);
		let final_diff = wait_diff(&core, run.run_id, DiffScope::Final).await;
		assert!(
			final_diff
				.patch
				.contains("-Initial dirty content\n+Partial second turn\n")
		);
		core.close().await;
		let (core, _) = start(dir.path()).await;
		let historical = wait_diff(
			&core,
			run.run_id,
			DiffScope::Historical {
				from_turn: 0,
				to_turn: 2,
			},
		)
		.await;
		assert_eq!(
			(&historical.before, &historical.after, &historical.patch),
			(&final_diff.before, &final_diff.after, &final_diff.patch)
		);
		std::fs::write(root.join("README.md"), "Current edit\n").unwrap();
		let current = wait_diff(&core, run.run_id, DiffScope::Current).await;
		assert!(
			current
				.patch
				.contains("-Initial dirty content\n+Current edit\n")
		);
	}

	#[tokio::test]
	async fn only_complete_content_evidence_attributes_user_terminal_and_harness_changes()
	 {
		let dir = tempfile::tempdir().unwrap();
		let (core, sender) = start(dir.path()).await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
		for path in
			["user.txt", "terminal.txt", "harness.txt", "unexplained.txt"]
		{
			std::fs::write(root.join(path), "Before\n").unwrap();
		}
		git(&root, &["add", "-A"]);
		git(&root, &["commit", "-qm", "Base"]);
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "Edit".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		let origins = [
			(
				"user.txt",
				ChangeOrigin::UserEdit {
					client_id: actor().client_id(),
				},
			),
			(
				"terminal.txt",
				ChangeOrigin::WorkspaceTerminal {
					terminal_id: uuid::Uuid::new_v4(),
				},
			),
			("harness.txt", ChangeOrigin::Harness { run_id: run.run_id }),
			(
				"unexplained.txt",
				ChangeOrigin::Harness { run_id: run.run_id },
			),
		];
		for (path, origin) in &origins {
			std::fs::write(root.join(path), "After\n").unwrap();
			let evidence = ChangeEvidence {
				activity_id: uuid::Uuid::new_v4().to_string(),
				origin: origin.clone(),
				path: (*path).into(),
				before_object: git(
					&root,
					&["rev-parse", &format!("HEAD:{path}")],
				)
				.trim()
				.into(),
				after_object: git(&root, &["hash-object", "--", path])
					.trim()
					.into(),
				before_mode: "100644".into(),
				after_mode: "100644".into(),
			};
			core.record_change_evidence(run.run_id, evidence)
				.await
				.unwrap();
		}
		std::fs::write(root.join("unexplained.txt"), "External overwrite\n")
			.unwrap();
		sender
			.send(RunObservation::Completed("native".into()))
			.await
			.unwrap();
		sender.send(RunObservation::Ended(Some(0))).await.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Completed).await;
		let diff =
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
		let actual: Vec<_> = diff
			.files
			.iter()
			.map(|file| (file.path.clone(), file.origin.clone()))
			.collect();
		let mut expected: Vec<_> = origins
			.into_iter()
			.map(|(p, o)| {
				(
					p.to_owned(),
					if p == "unexplained.txt" {
						ChangeOrigin::ExternalOrUnknown
					} else {
						o
					},
				)
			})
			.collect();
		expected.sort_by(|a, b| a.0.cmp(&b.0));
		assert_eq!(actual, expected);
		core.close().await;
		let (core, _) = start(dir.path()).await;
		assert_eq!(
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await,
			diff
		);
		let final_diff = wait_diff(&core, run.run_id, DiffScope::Final).await;
		assert_eq!(final_diff.files, diff.files);
	}

	#[tokio::test]
	async fn a_direct_edit_records_user_evidence_for_the_active_turn() {
		let dir = tempfile::tempdir().unwrap();
		let (core, sender) = start(dir.path()).await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
		std::fs::write(root.join("notes.md"), "Before\n").unwrap();
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "Edit".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		let target = FileTarget::Project { project_id };
		let QueryResult::EditableFile(file) = core
			.query(
				&actor(),
				Query::EditableFile {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Editable file")
		};
		let CommandOutcome::UserEditApplied(_) = core
			.execute(
				&actor(),
				request(Command::ApplyUserEdit {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
					expected_revision: file.revision,
					content: "After\n".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("User edit")
		};
		sender
			.send(RunObservation::Completed("native".into()))
			.await
			.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 1,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		let diff =
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
		let QueryResult::RunExecution(execution) = core
			.query(&actor(), Query::RunExecution { run_id: run.run_id })
			.await
			.unwrap()
		else {
			panic!("Run execution")
		};
		assert_eq!(execution.run.lifecycle, RunLifecycle::Active);
		let edited = diff
			.files
			.iter()
			.find(|file| file.path == "notes.md")
			.expect("edited file");
		assert_eq!(
			edited.origin,
			ChangeOrigin::UserEdit {
				client_id: actor().client_id(),
			}
		);
		let QueryResult::EditableFile(file) = core
			.query(
				&actor(),
				Query::EditableFile {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Editable file")
		};
		let CommandOutcome::UserEditApplied(_) = core
			.execute(
				&actor(),
				request(Command::ApplyUserEdit {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
					expected_revision: file.revision,
					content: "Between turns\n".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("User edit between turns")
		};
		assert_eq!(
			std::fs::read_to_string(root.join("notes.md")).unwrap(),
			"Between turns\n"
		);
		let between_turn_evidence = core
			.store
			.read(async |tx| {
				let execution = tx.run_execution(run.run_id.0).await?.unwrap();
				let state: crate::run::state::State =
					crate::run::state::decode(&execution.state)?;
				Ok::<_, CoreError>(state.changes.unwrap().between_turn_evidence)
			})
			.await
			.unwrap();
		assert_eq!(between_turn_evidence.len(), 1);
		let current = wait_diff(&core, run.run_id, DiffScope::Current).await;
		assert_eq!(
			current
				.files
				.iter()
				.find(|file| file.path == "notes.md")
				.expect("current direct edit")
				.origin,
			ChangeOrigin::UserEdit {
				client_id: actor().client_id(),
			}
		);
		sender.send(RunObservation::TurnStarted).await.unwrap();
		sender
			.send(RunObservation::TurnEnded(TurnOutcome::Completed))
			.await
			.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		let subsequent = wait_diff(
			&core,
			run.run_id,
			DiffScope::Historical {
				from_turn: 0,
				to_turn: 2,
			},
		)
		.await;
		assert_eq!(
			subsequent
				.files
				.iter()
				.find(|file| file.path == "notes.md")
				.expect("direct edit retained by a later checkpoint")
				.origin,
			ChangeOrigin::UserEdit {
				client_id: actor().client_id(),
			}
		);
		let QueryResult::EditableFile(file) = core
			.query(
				&actor(),
				Query::EditableFile {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Editable file")
		};
		let CommandOutcome::UserEditApplied(_) = core
			.execute(
				&actor(),
				request(Command::ApplyUserEdit {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
					expected_revision: file.revision,
					content: "Terminal gap\n".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("User edit before terminal boundary")
		};
		sender.send(RunObservation::Ended(Some(0))).await.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 3,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Completed).await;
		let final_diff = wait_diff(&core, run.run_id, DiffScope::Final).await;
		assert_eq!(
			final_diff
				.files
				.iter()
				.find(|file| file.path == "notes.md")
				.expect("terminal-gap direct edit")
				.origin,
			ChangeOrigin::UserEdit {
				client_id: actor().client_id(),
			}
		);
	}

	#[tokio::test]
	async fn a_direct_edit_intent_finishes_after_restart_before_receipt_replay()
	{
		let dir = tempfile::tempdir().unwrap();
		let (core, _sender) = start(dir.path()).await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
		std::fs::write(root.join("notes.md"), "Before\n").unwrap();
		std::fs::write(root.join("external.md"), "Before\n").unwrap();
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "Edit".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		let target = FileTarget::Project { project_id };
		let QueryResult::EditableFile(file) = core
			.query(
				&actor(),
				Query::EditableFile {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Editable file")
		};
		let command_id = crate::test_support::command_id();
		let command = Command::ApplyUserEdit {
			target,
			path: RelativePath::parse("notes.md").unwrap(),
			expected_revision: file.revision.clone(),
			content: "After\n".into(),
		};
		let envelope =
			crate::test_support::request_with_id(command_id, command);
		crate::user_input::prepare(
			&core,
			crate::user_input::IntentContext {
				actor: &actor(),
				command_id,
				request_digest: envelope.request_digest(),
				recorded_at_unix_ms: core.now_unix_ms(),
			},
			target,
			RelativePath::parse("notes.md").unwrap(),
			file.revision,
			"After\n".into(),
		)
		.await
		.unwrap();
		assert_eq!(
			std::fs::read_to_string(root.join("notes.md")).unwrap(),
			"Before\n"
		);
		let QueryResult::EditableFile(external) = core
			.query(
				&actor(),
				Query::EditableFile {
					target,
					path: RelativePath::parse("external.md").unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Editable file")
		};
		let external_command_id = crate::test_support::command_id();
		let external_command = Command::ApplyUserEdit {
			target,
			path: RelativePath::parse("external.md").unwrap(),
			expected_revision: external.revision.clone(),
			content: "After\n".into(),
		};
		let external_envelope = crate::test_support::request_with_id(
			external_command_id,
			external_command,
		);
		crate::user_input::prepare(
			&core,
			crate::user_input::IntentContext {
				actor: &actor(),
				command_id: external_command_id,
				request_digest: external_envelope.request_digest(),
				recorded_at_unix_ms: core.now_unix_ms(),
			},
			target,
			RelativePath::parse("external.md").unwrap(),
			external.revision,
			"After\n".into(),
		)
		.await
		.unwrap();
		std::fs::write(root.join("external.md"), "After\n").unwrap();
		core.close().await;

		let (restarted, _) = start(dir.path()).await;
		restarted.perform_user_edits().await.unwrap();
		assert_eq!(
			std::fs::read_to_string(root.join("notes.md")).unwrap(),
			"After\n"
		);
		assert_eq!(
			std::fs::read_to_string(root.join("external.md")).unwrap(),
			"After\n"
		);
		let CommandOutcome::UserEditApplied(replayed) =
			restarted.execute(&actor(), envelope).await.unwrap()
		else {
			panic!("replayed user edit")
		};
		let QueryResult::EditableFile(opened) = restarted
			.query(
				&actor(),
				Query::EditableFile {
					target,
					path: RelativePath::parse("notes.md").unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Editable file")
		};
		assert_eq!(replayed.revision, opened.revision);
		let CommandOutcome::UserEditApplied(_) = restarted
			.execute(&actor(), external_envelope)
			.await
			.unwrap()
		else {
			panic!("replayed externally satisfied edit")
		};
		let QueryResult::Events(events) = restarted
			.query(
				&actor(),
				Query::Events {
					after: EventSequence(0),
				},
			)
			.await
			.unwrap()
		else {
			panic!("Events")
		};
		let evidence_paths = events
			.events
			.into_iter()
			.filter_map(|event| match event.kind {
				EventKind::ChangeEvidenceRecorded { evidence, .. } => {
					Some(evidence.path)
				}
				_ => None,
			})
			.collect::<Vec<_>>();
		assert_eq!(evidence_paths, vec!["notes.md"]);
	}

	#[tokio::test]
	async fn large_patches_remain_readable_in_bounded_chunks_without_following_links()
	 {
		let dir = tempfile::tempdir().unwrap();
		let (core, sender) = start(dir.path()).await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "Edit".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		for i in 0..300 {
			std::fs::write(root.join(format!("file-{i:03}.txt")), "new")
				.unwrap();
		}
		let content = format!("{}\n", "Large diff ".repeat(20000));
		std::fs::write(root.join("large.txt"), &content).unwrap();
		sender
			.send(RunObservation::Completed("native".into()))
			.await
			.unwrap();
		sender.send(RunObservation::Ended(Some(0))).await.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Completed).await;
		let diff =
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
		assert!(diff.patch_truncated);
		assert_eq!(diff.total_files, 301);
		// An unrelated Event must not strand immutable pages on a busy Plane.
		core.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::LocalCheckout { project_id },
			}),
		)
		.await
		.unwrap();
		let mut file_count = diff.files.len();
		let mut next = diff.next_page;
		while let Some(cursor) = next {
			let QueryResult::ChangeDiff(page) = core
				.query(&actor(), Query::NextChangeDiff { cursor })
				.await
				.unwrap()
			else {
				panic!("page")
			};
			assert_eq!(page.cursor, diff.cursor);
			file_count += page.files.len();
			next = page.next_page;
		}
		assert_eq!(file_count, 301);
		let mut bytes = Vec::new();
		loop {
			let QueryResult::ChangeArtifact(chunk) = core
				.query(
					&actor(),
					Query::ChangeArtifact {
						sha256: diff.artifact.sha256.clone(),
						offset: bytes.len() as u64,
					},
				)
				.await
				.unwrap()
			else {
				panic!("Artifact")
			};
			assert!(chunk.bytes.len() <= 65536);
			bytes.extend(chunk.bytes);
			if bytes.len() as u64 == chunk.artifact.size {
				break;
			}
		}
		assert!(
			String::from_utf8(bytes.clone())
				.unwrap()
				.contains(&format!("+{content}"))
		);
		use sha2::{Digest, Sha256};
		assert_eq!(
			format!("{:x}", Sha256::digest(&bytes)),
			diff.artifact.sha256
		);
		let current = wait_diff(&core, run.run_id, DiffScope::Current).await;
		let cursor = current.next_page.unwrap();
		std::fs::write(root.join("aaa-inserted.txt"), "Changed between pages")
			.unwrap();
		assert_eq!(
			core.query(&actor(), Query::NextChangeDiff { cursor })
				.await
				.unwrap_err()
				.code,
			"pagination.stale"
		);
		let cursor = diff.next_page.unwrap();
		core.close().await;
		let (core, _) = start(dir.path()).await;
		assert_eq!(
			core.query(&actor(), Query::NextChangeDiff { cursor })
				.await
				.unwrap_err()
				.code,
			"pagination.stale"
		);
		let artifact_path =
			dir.path().join("artifacts").join(&diff.artifact.sha256);
		let outside = dir.path().join("private.txt");
		std::fs::write(&outside, "private").unwrap();
		std::fs::remove_file(&artifact_path).unwrap();
		std::os::unix::fs::symlink(&outside, &artifact_path).unwrap();
		assert!(
			core.query(
				&actor(),
				Query::ChangeArtifact {
					sha256: diff.artifact.sha256,
					offset: 0
				}
			)
			.await
			.is_err()
		);
		assert_eq!(std::fs::read_to_string(outside).unwrap(), "private");
	}

	async fn wait_diff(
		core: &Core,
		run_id: RunId,
		scope: DiffScope,
	) -> Box<ChangeDiff> {
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				if let Ok(QueryResult::ChangeDiff(diff)) = core
					.query(
						&actor(),
						Query::ChangeDiff {
							run_id,
							scope: scope.clone(),
						},
					)
					.await
				{
					break diff;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap()
	}

	async fn wait_for(core: &Core, run_id: RunId, lifecycle: RunLifecycle) {
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				if let QueryResult::RunExecution(state) = core
					.query(&actor(), Query::RunExecution { run_id })
					.await
					.unwrap() && state.run.lifecycle == lifecycle
				{
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
	}

	async fn start(home: &Path) -> (Arc<Core>, mpsc::Sender<RunObservation>) {
		let (core, sender, _) = start_answering(home).await;
		(core, sender)
	}

	/// Every decision this fixture's Craft was sent, in order.
	pub(crate) type Answered =
		Arc<std::sync::Mutex<Vec<(String, crate::ReviewDecision)>>>;

	/// The same fixture, keeping what Core answered held approval requests with.
	pub(crate) async fn start_answering(
		home: &Path,
	) -> (Arc<Core>, mpsc::Sender<RunObservation>, Answered) {
		let (sender, receiver) = mpsc::channel(32);
		let answered = Answered::default();
		let core = crate::test_support::start_core(&home.join("plane.sqlite3"))
			.await
			.with_run_host(Arc::new(Host(
				Mutex::new(Some(receiver)),
				Arc::clone(&answered),
			)));
		(Arc::new(core), sender, answered)
	}

	#[derive(Debug)]
	struct Host(Mutex<Option<mpsc::Receiver<RunObservation>>>, Answered);
	impl RunHost for Host {
		/// The fixture Craft stands in for a Harness whose native Provider the
		/// host knows, which is what a Visa Run and its reviewer need.
		fn native_provider(
			&self,
			_craft: &PinnedCraft,
		) -> Result<ProviderId, CoreError> {
			Ok(ProviderId("anthropic".into()))
		}
		fn prepare_next_run(
			&self,
			plan: LaunchPlan,
		) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
			Box::pin(async { Ok(plan) })
		}
		fn pin(
			&self,
			_home: std::path::PathBuf,
			_id: String,
		) -> RunFuture<'_, Result<PinnedCraft, CoreError>> {
			Box::pin(async {
				use sha2::{Digest, Sha256};
				let executable = Path::new("/bin/cat").canonicalize().unwrap();
				Ok(PinnedCraft {
					id: "fake".into(),
					sha256: format!(
						"{:x}",
						Sha256::digest(std::fs::read(&executable).unwrap())
					),
					executable,
					adapter_state: "fixture".into(),
				})
			})
		}
		fn start(
			&self,
			_home: std::path::PathBuf,
			_run_id: RunId,
			_plan: LaunchPlan,
		) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>> {
			Box::pin(async {
				Ok(Box::new(Connection {
					receiver: Mutex::new(self.0.lock().await.take().unwrap()),
					started: std::sync::atomic::AtomicBool::new(false),
					answered: Arc::clone(&self.1),
				}) as Box<dyn RunConnection>)
			})
		}
	}
	struct Connection {
		receiver: Mutex<mpsc::Receiver<RunObservation>>,
		started: std::sync::atomic::AtomicBool,
		answered: Answered,
	}
	impl RunConnection for Connection {
		fn submit_turn(
			&self,
			_turn_id: uuid::Uuid,
			_prompt: String,
			_child_work: crate::ChildWork,
		) -> RunFuture<'_, Result<(), CoreError>> {
			Box::pin(async { panic!("no input queued in this fixture") })
		}
		#[expect(
			clippy::await_holding_invalid_type,
			reason = "the shared fixture port has one serialized observation receiver"
		)]
		fn receive(&self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
			Box::pin(async move {
				if !self.started.swap(true, std::sync::atomic::Ordering::SeqCst)
				{
					return Ok(RunObservation::Started {
						helper_pid: 100,
						harness_pid: 101,
					});
				}
				Ok(self.receiver.lock().await.recv().await.unwrap())
			})
		}
		fn supports_native_cancellation(&self) -> bool {
			false
		}
		fn interrupt(
			&self,
			_turn_id: uuid::Uuid,
		) -> RunFuture<'_, Result<(), CoreError>> {
			Box::pin(async { panic!("no cancellation in this fixture") })
		}
		fn decide_approval<'a>(
			&'a self,
			request_id: &'a str,
			decision: crate::ReviewDecision,
		) -> RunFuture<'a, Result<(), CoreError>> {
			Box::pin(async move {
				self.answered
					.lock()
					.expect("answered lock")
					.push((request_id.to_owned(), decision));
				Ok(())
			})
		}
		fn acknowledge(
			&self,
			_offset: u64,
		) -> RunFuture<'_, Result<(), CoreError>> {
			Box::pin(async { Ok(()) })
		}
		fn finish(&self) -> RunFuture<'_, Result<(), CoreError>> {
			Box::pin(async { Ok(()) })
		}
	}

	mod regression {
		use super::*;
		use pretty_assertions::assert_eq;

		async fn active_run(core: &Arc<Core>, root: &Path) -> RunId {
			let project_id = register_repository(core, root).await;
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
				panic!("Conversation")
			};
			let CommandOutcome::RunCreated(run) = core
				.execute(
					&actor(),
					request(Command::StartRun {
						conversation_id: conversation.conversation_id,
						craft: "fake".into(),
						prompt: "Edit".into(),
					}),
				)
				.await
				.unwrap()
			else {
				panic!("Run")
			};
			core.perform_runs().await.unwrap();
			wait_for(core, run.run_id, RunLifecycle::Active).await;
			run.run_id
		}

		fn edit(root: &Path, run_id: RunId, text: &str) -> ChangeEvidence {
			let before =
				git(root, &["hash-object", "README.md"]).trim().to_owned();
			std::fs::write(root.join("README.md"), text).unwrap();
			ChangeEvidence {
				activity_id: uuid::Uuid::new_v4().to_string(),
				origin: ChangeOrigin::Harness { run_id },
				path: "README.md".into(),
				before_object: before,
				after_object: git(root, &["hash-object", "README.md"])
					.trim()
					.into(),
				before_mode: "100644".into(),
				after_mode: "100644".into(),
			}
		}

		async fn progress(sender: &mpsc::Sender<RunObservation>, offset: u64) {
			sender
				.send(RunObservation::Progress {
					offset,
					checkpoint: String::new(),
				})
				.await
				.unwrap();
		}

		#[tokio::test]
		async fn excess_or_conflicting_receipts_preserve_a_completed_unknown_checkpoint()
		 {
			for conflict in [false, true] {
				let dir = tempfile::tempdir().unwrap();
				let (core, sender) = start(dir.path()).await;
				let root = dir.path().join("repo");
				let run_id = active_run(&core, &root).await;
				let first = edit(&root, run_id, "First\n");
				core.record_change_evidence(run_id, first.clone())
					.await
					.unwrap();
				if conflict {
					let mut second = edit(&root, run_id, "Conflicting\n");
					second.activity_id = first.activity_id;
					core.record_change_evidence(run_id, second).await.unwrap();
				} else {
					for index in 0..256 {
						core.record_change_evidence(
							run_id,
							edit(&root, run_id, &format!("Edit {index}\n")),
						)
						.await
						.unwrap();
					}
				}
				sender
					.send(RunObservation::Completed("native".into()))
					.await
					.unwrap();
				sender.send(RunObservation::Ended(Some(0))).await.unwrap();
				progress(&sender, 2).await;
				wait_for(&core, run_id, RunLifecycle::Completed).await;
				let diff = wait_diff(&core, run_id, DiffScope::Final).await;
				assert_eq!(
					diff.files[0].origin,
					ChangeOrigin::ExternalOrUnknown
				);
				assert!(!diff.patch.is_empty());
			}
		}

		#[tokio::test]
		async fn aggregate_scopes_cannot_upgrade_unknown_intermediate_changes()
		{
			let dir = tempfile::tempdir().unwrap();
			let (core, sender) = start(dir.path()).await;
			let root = dir.path().join("repo");
			let run_id = active_run(&core, &root).await;
			core.record_change_evidence(
				run_id,
				edit(&root, run_id, "Harness B\n"),
			)
			.await
			.unwrap();
			std::fs::write(root.join("README.md"), "External C\n").unwrap();
			sender
				.send(RunObservation::TurnEnded(TurnOutcome::Completed))
				.await
				.unwrap();
			progress(&sender, 2).await;
			let first =
				wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
			assert_eq!(first.files[0].origin, ChangeOrigin::ExternalOrUnknown);
			sender.send(RunObservation::TurnStarted).await.unwrap();
			sender
				.send(RunObservation::Activity(RunActivity::WaitingForUser))
				.await
				.unwrap();
			progress(&sender, 3).await;
			tokio::time::timeout(std::time::Duration::from_secs(10), async {
				loop {
					if let QueryResult::RunExecution(state) = core
						.query(&actor(), Query::RunExecution { run_id })
						.await
						.unwrap() && state.activity
						== Some(RunActivity::WaitingForUser)
					{
						break;
					}
					tokio::time::sleep(std::time::Duration::from_millis(10))
						.await;
				}
			})
			.await
			.unwrap();
			std::fs::write(root.join("README.md"), "Harness B\n").unwrap();
			let current = wait_diff(&core, run_id, DiffScope::Current).await;
			assert_eq!(
				current.files[0].origin,
				ChangeOrigin::ExternalOrUnknown
			);
			sender
				.send(RunObservation::Completed("native".into()))
				.await
				.unwrap();
			sender.send(RunObservation::Ended(Some(0))).await.unwrap();
			progress(&sender, 4).await;
			wait_for(&core, run_id, RunLifecycle::Completed).await;
			core.close().await;
			let (core, _) = start(dir.path()).await;
			for scope in [
				DiffScope::Final,
				DiffScope::Historical {
					from_turn: 0,
					to_turn: 2,
				},
			] {
				let diff = wait_diff(&core, run_id, scope).await;
				assert_eq!(
					diff.files[0].origin,
					ChangeOrigin::ExternalOrUnknown
				);
			}
		}

		#[tokio::test]
		async fn artifact_reservations_deduplicate_and_enforce_the_run_budget_across_restart()
		 {
			let dir = tempfile::tempdir().unwrap();
			let (core, sender) = start(dir.path()).await;
			let root = dir.path().join("repo");
			let run_id = active_run(&core, &root).await;
			std::fs::write(root.join("README.md"), "First patch\n").unwrap();
			let first = wait_diff(&core, run_id, DiffScope::Current).await;
			let budget = dir
				.path()
				.join("artifacts")
				.join(format!(".run-{}.budget", run_id.0));
			let reserved = std::fs::read(&budget).unwrap();
			assert_eq!(
				u64::from_be_bytes(reserved.clone().try_into().unwrap()),
				first.artifact.size
			);
			assert_eq!(
				wait_diff(&core, run_id, DiffScope::Current).await,
				first
			);
			assert_eq!(std::fs::read(&budget).unwrap(), reserved);
			// Lower the durable policy through Core; checkpoint queries and completed
			// turns must honor it, including while using their existing store transaction.
			core.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::ArtifactRunMiB,
					scope: SettingScope::Plane,
					value: SettingValue::Count(0),
				}),
			)
			.await
			.unwrap();
			std::fs::write(root.join("README.md"), "Another distinct patch\n")
				.unwrap();
			let limited = wait_diff(&core, run_id, DiffScope::Current).await;
			assert_eq!(
				limited.artifact.availability,
				ArtifactAvailability::RunBudgetExceeded
			);
			assert!(limited.patch_truncated);
			sender
				.send(RunObservation::Completed("native".into()))
				.await
				.unwrap();
			sender.send(RunObservation::Ended(Some(0))).await.unwrap();
			progress(&sender, 2).await;
			wait_for(&core, run_id, RunLifecycle::Completed).await;
			let completed =
				wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
			assert_eq!(
				completed.artifact.availability,
				ArtifactAvailability::RunBudgetExceeded
			);
			assert_eq!(completed.files[0].path, "README.md");
			core.close().await;
			let (core, _) = start(dir.path()).await;
			assert_eq!(
				wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await,
				completed
			);
			let QueryResult::ChangeArtifact(chunk) = core
				.query(
					&actor(),
					Query::ChangeArtifact {
						sha256: first.artifact.sha256.clone(),
						offset: 0,
					},
				)
				.await
				.unwrap()
			else {
				panic!("Artifact")
			};
			assert_eq!(chunk.artifact, first.artifact);
			assert!(
				!std::fs::read_dir(dir.path().join("artifacts"))
					.unwrap()
					.any(|entry| entry
						.unwrap()
						.file_name()
						.to_string_lossy()
						.starts_with(".pending-"))
			);
		}

		#[tokio::test]
		async fn oversized_files_keep_metadata_without_entering_git_or_blocking_turns()
		 {
			let dir = tempfile::tempdir().unwrap();
			let (core, sender) = start(dir.path()).await;
			core.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::ArtifactMaxMiB,
					scope: SettingScope::Plane,
					value: SettingValue::Count(1),
				}),
			)
			.await
			.unwrap();
			let root = dir.path().join("repo");
			let run_id = active_run(&core, &root).await;
			let size = 2_u64 * 1024 * 1024;
			std::fs::File::create(root.join("large.bin"))
				.unwrap()
				.set_len(size)
				.unwrap();
			std::fs::write(root.join("README.md"), "Small edit\n").unwrap();
			sender
				.send(RunObservation::Completed("native".into()))
				.await
				.unwrap();
			sender.send(RunObservation::Ended(Some(0))).await.unwrap();
			progress(&sender, 2).await;
			wait_for(&core, run_id, RunLifecycle::Completed).await;
			let diff =
				wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
			assert_eq!(
				diff.after.omitted_files,
				vec![OmittedFile {
					path: "large.bin".into(),
					size,
					mode: "100644".into()
				}]
			);
			assert_eq!(
				diff.files
					.iter()
					.find(|file| file.path == "large.bin")
					.unwrap(),
				&ChangedFile {
					path: "large.bin".into(),
					before_size: None,
					after_size: Some(size),
					before_object: Some("0".repeat(40)),
					after_object: None,
					before_mode: "000000".into(),
					after_mode: "100644".into(),
					origin: ChangeOrigin::ExternalOrUnknown,
				}
			);
			assert!(diff.patch.contains("+Small edit"));
			assert!(
				!git(&root, &["ls-tree", "-r", &diff.after.tree])
					.contains("large.bin")
			);
			assert_eq!(
				std::fs::metadata(root.join("large.bin")).unwrap().len(),
				size
			);
			core.close().await;
			let (core, _) = start(dir.path()).await;
			assert_eq!(
				wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await,
				diff
			);
		}

		#[tokio::test]
		async fn crossing_the_size_limit_preserves_each_captured_side_and_same_boundary_is_empty()
		 {
			let dir = tempfile::tempdir().unwrap();
			let (core, sender) = start(dir.path()).await;
			let root = dir.path().join("repo");
			let run_id = active_run(&core, &root).await;
			let original = git(&root, &["rev-parse", "HEAD:README.md"])
				.trim()
				.to_owned();
			std::fs::File::create(root.join("README.md"))
				.unwrap()
				.set_len(513 * 1024 * 1024)
				.unwrap();
			sender
				.send(RunObservation::TurnEnded(TurnOutcome::Completed))
				.await
				.unwrap();
			progress(&sender, 2).await;
			let first =
				wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
			assert_eq!(
				(
					&first.files[0].before_object,
					&first.files[0].after_object,
					first.files[0].before_mode.as_str(),
					first.files[0].after_mode.as_str()
				),
				(&Some(original), &None, "100644", "100644")
			);
			let same = wait_diff(
				&core,
				run_id,
				DiffScope::Historical {
					from_turn: 1,
					to_turn: 1,
				},
			)
			.await;
			assert_eq!((same.total_files, same.files), (0, vec![]));
			sender.send(RunObservation::TurnStarted).await.unwrap();
			sender
				.send(RunObservation::Activity(RunActivity::WaitingForUser))
				.await
				.unwrap();
			progress(&sender, 3).await;
			tokio::time::timeout(std::time::Duration::from_secs(10), async {
				loop {
					if let QueryResult::RunExecution(state) = core
						.query(&actor(), Query::RunExecution { run_id })
						.await
						.unwrap() && state.activity
						== Some(RunActivity::WaitingForUser)
					{
						break;
					}
					tokio::time::sleep(std::time::Duration::from_millis(10))
						.await;
				}
			})
			.await
			.unwrap();
			std::fs::write(root.join("README.md"), "Small again\n").unwrap();
			let after =
				git(&root, &["hash-object", "README.md"]).trim().to_owned();
			sender
				.send(RunObservation::Completed("second-turn".into()))
				.await
				.unwrap();
			sender.send(RunObservation::Ended(Some(0))).await.unwrap();
			progress(&sender, 4).await;
			wait_for(&core, run_id, RunLifecycle::Completed).await;
			let second =
				wait_diff(&core, run_id, DiffScope::Turn { turn: 2 }).await;
			assert_eq!(
				(
					&second.files[0].before_object,
					&second.files[0].after_object,
					second.files[0].before_mode.as_str(),
					second.files[0].after_mode.as_str()
				),
				(&None, &Some(after), "100644", "100644")
			);
		}

		mod disk_pressure {
			use super::*;
			use pretty_assertions::assert_eq;

			#[tokio::test]
			async fn current_diff_remains_readable_when_pressure_prevents_artifact_ingestion()
			 {
				let dir = tempfile::tempdir().unwrap();
				let (core, _sender) = start(dir.path()).await;
				let root = dir.path().join("repo");
				let run_id = active_run(&core, &root).await;
				std::fs::write(root.join("README.md"), "Protected work\n")
					.unwrap();
				let objects_before = git(&root, &["count-objects", "-v"]);
				let reader = crate::test_support::start_core(
					&dir.path().join("plane.sqlite3"),
				)
				.await
				.with_artifact_limits(ArtifactLimits {
					free_reserve_bytes: u64::MAX,
					..Default::default()
				});
				let QueryResult::ChangeDiff(diff) = reader
					.query(
						&actor(),
						Query::ChangeDiff {
							run_id,
							scope: DiffScope::Current,
						},
					)
					.await
					.unwrap()
				else {
					panic!("Diff")
				};
				assert_eq!(
					git(&root, &["count-objects", "-v"]),
					objects_before
				);
				assert_eq!(
					diff.artifact.availability,
					ArtifactAvailability::DiskPressure
				);
				assert_eq!(
					std::fs::read_to_string(root.join("README.md")).unwrap(),
					"Protected work\n"
				);
			}

			#[tokio::test]
			async fn query_only_patches_obey_a_zero_disposable_budget() {
				let dir = tempfile::tempdir().unwrap();
				let (core, _sender) = start(dir.path()).await;
				let root = dir.path().join("repo");
				let run_id = active_run(&core, &root).await;
				core.execute(
					&actor(),
					request(Command::SetSetting {
						key: SettingKey::StorageDisposableMiB,
						scope: SettingScope::Plane,
						value: SettingValue::Count(0),
					}),
				)
				.await
				.unwrap();
				std::fs::write(root.join("README.md"), "Disposable diff\n")
					.unwrap();
				let QueryResult::ChangeDiff(diff) = core
					.query(
						&actor(),
						Query::ChangeDiff {
							run_id,
							scope: DiffScope::Current,
						},
					)
					.await
					.unwrap()
				else {
					panic!("Diff")
				};
				assert_eq!(
					diff.artifact.availability,
					ArtifactAvailability::DiskPressure
				);
				assert_eq!(
					std::fs::read_dir(dir.path().join("cache"))
						.unwrap()
						.count(),
					0
				);
			}
		}
	}

	#[tokio::test]
	async fn utility_git_text_uses_only_the_requested_checkpoint_and_plane_instructions()
	 {
		use crate::utility::tests::{Inference, configured, generate, setting};
		let dir = tempfile::tempdir().unwrap();
		let (core, sender) = start(dir.path()).await;
		let peer = Arc::new(Inference {
			calls: std::sync::Mutex::default(),
			output:
				r#"{"subject":"Update README","body":"Describe the change."}"#
					.into(),
		});
		let core = Arc::new(
			Arc::try_unwrap(core)
				.unwrap()
				.with_utility_host(peer.clone()),
		);
		let binding = configured(&core).await;
		setting(
			&core,
			SettingKey::UtilityContentConsent,
			SettingValue::Text(binding.binding_id.0.to_string()),
		)
		.await;
		setting(
			&core,
			SettingKey::GitMessageInstructions,
			SettingValue::Text("Use imperative subjects".into()),
		)
		.await;
		let root = dir.path().join("repo");
		let project_id = register_repository(&core, &root).await;
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::StartRun {
					conversation_id: conversation.conversation_id,
					craft: "fake".into(),
					prompt: "UNRELATED OPENING CONTENT".into(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.perform_runs().await.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Active).await;
		std::fs::write(root.join("README.md"), "Turn one\n").unwrap();
		sender
			.send(RunObservation::Completed("native-1".into()))
			.await
			.unwrap();
		sender.send(RunObservation::Ended(Some(0))).await.unwrap();
		sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		wait_for(&core, run.run_id, RunLifecycle::Completed).await;
		let input = UtilityRequest::GitText {
			run_id: run.run_id,
			turn: 1,
		};
		let disabled = generate(&core, input.clone()).await;
		assert_eq!(
			disabled.outcome,
			UtilityOutcome::Text {
				text: "Update Run changes (turn 1)".into(),
				body: String::new(),
				fallback_reason: Some("utility.disabled".into())
			}
		);
		assert!(peer.calls.lock().unwrap().is_empty());
		setting(&core, SettingKey::UtilityGitText, SettingValue::Flag(true))
			.await;
		let diff =
			wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
		std::fs::write(root.join("README.md"), "UNRELATED LIVE EDIT\n")
			.unwrap();
		let job = generate(&core, input).await;
		assert_eq!(
			job.outcome,
			UtilityOutcome::Text {
				text: "Update README".into(),
				body: "Describe the change.".into(),
				fallback_reason: None
			}
		);
		assert_eq!(
			*peer.calls.lock().unwrap(),
			vec![UtilityInput::GitText {
				patch: diff.patch,
				instructions: "Use imperative subjects".into()
			}]
		);
	}
}
