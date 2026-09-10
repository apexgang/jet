//! Non-destructive Git delivery vocabulary (ADR-0029, ADR-0067).
pub(crate) mod github;
pub(crate) mod github_host;
pub(crate) mod io;
pub(crate) mod state;
pub(crate) mod work;

use crate::{ConversationId, RunId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The only Git mutations exposed by delivery. Each is also a manual Command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitOperation {
	/// Create and check out a new branch; never replace an existing branch.
	Branch {
		/// Editable proposed name.
		name: String,
	},
	/// Commit the exact retained checkpoint tree.
	Commit,
	/// Push one branch without force, tags, deletion, or configured refspecs.
	Push {
		/// One configured remote name.
		remote: String,
	},
	/// Create or update this Conversation's GitHub draft.
	DraftPullRequest {
		/// One configured GitHub remote name.
		remote: String,
		/// Explicit base branch, or the GitHub repository default.
		base: Option<String>,
	},
}
/// Immutable source of generated Git text and committed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCheckpoint {
	/// Owning Run.
	pub run_id: RunId,
	/// Turn number, starting at one.
	pub turn: u32,
}
/// Policy resolved at admission and revalidated before each Effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDeliveryPolicy {
	/// Whether a successful checkpoint admitted this operation.
	pub automatic: bool,
	/// Automatic branch creation.
	pub branch: bool,
	/// Automatic commits.
	pub commit: bool,
	/// Automatic pushes.
	pub push: bool,
	/// Automatic draft creation and updates.
	pub draft_pull_request: bool,
	/// Editable branch prefix.
	pub branch_prefix: String,
}
/// Individually durable outcome; a later failure never rolls it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GitDeliveryOutcome {
	/// Committed request awaiting execution.
	Pending,
	/// Observed Git state after completion.
	Completed {
		/// Resulting commit identity.
		head: String,
		/// Resulting branch; absent for detached commits.
		branch: Option<String>,
		/// Hosted draft URL, when applicable.
		pull_request: Option<String>,
	},
	/// A definite refusal or unsuccessful operation.
	Failed {
		/// Stable, content-free explanation.
		code: String,
	},
	/// Reconciliation could not establish what happened. Never retried.
	OutcomeUnknown,
}
/// Exact generated text used by a commit or draft Effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitMessage {
	/// Commit subject or PR title.
	pub title: String,
	/// Commit or PR body before Jet attribution markers.
	pub body: String,
	/// Stable reason deterministic text replaced Utility inference.
	pub fallback_reason: Option<String>,
}
/// One retained delivery operation, its exact policy, text attribution and outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDelivery {
	/// Exact bounded text, absent for branch and push operations.
	pub message: Option<GitMessage>,
	/// User who acknowledged an uncertain outcome, without retrying it.
	pub acknowledged_by: Option<crate::ClientId>,
	/// Stable Effect identity.
	pub delivery_id: Uuid,
	/// Owning Conversation.
	pub conversation_id: ConversationId,
	/// Checkpoint supplying content, absent for manual branch and push Commands.
	pub checkpoint: Option<GitCheckpoint>,
	/// Explicit allowlisted operation.
	pub operation: GitOperation,
	/// Exact automation policy at admission.
	pub policy: GitDeliveryPolicy,
	/// Bounded Utility job containing the exact text and inference attribution.
	pub utility_job: Option<Uuid>,
	/// Latest durable outcome.
	pub outcome: GitDeliveryOutcome,
}

#[cfg(test)]
pub(crate) mod tests {
	use crate::test_support::{
		actor, git, register_repository, request, start_core,
	};
	use crate::*;
	use pretty_assertions::assert_eq;

	async fn deliveries(core: &Core, id: ConversationId) -> Vec<GitDelivery> {
		let QueryResult::GitDeliveries(value) = core
			.query(
				&actor(),
				Query::GitDeliveries {
					conversation_id: id,
				},
			)
			.await
			.unwrap()
		else {
			panic!("deliveries")
		};
		value
	}

	#[tokio::test]
	async fn manual_branch_remains_available_with_automation_off_and_is_idempotent()
	 {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let core = start_core(&path).await;
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
			panic!("conversation")
		};
		let id = conversation.conversation_id;
		let head = git(&root, &["rev-parse", "HEAD"]);
		let command = request(Command::DeliverGit {
			conversation_id: id,
			checkpoint: None,
			operation: GitOperation::Branch {
				name: "feature/manual".into(),
			},
		});
		let admitted = core.execute(&actor(), command.clone()).await.unwrap();
		assert_eq!(git(&root, &["branch", "--show-current"]).trim(), "main");
		core.perform_git_deliveries().await.unwrap();
		let result = deliveries(&core, id).await;
		assert_eq!(
			result[0].outcome,
			GitDeliveryOutcome::Completed {
				head: head.trim().into(),
				branch: Some("feature/manual".into()),
				pull_request: None
			}
		);
		core.close().await;
		let core = start_core(&path).await;
		assert_eq!(core.execute(&actor(), command).await.unwrap(), admitted);
		core.perform_git_deliveries().await.unwrap();
		assert_eq!(deliveries(&core, id).await, result);
	}

	struct TurnFixture {
		core: std::sync::Arc<Core>,
		sender: tokio::sync::mpsc::Sender<RunObservation>,
		root: std::path::PathBuf,
		conversation_id: ConversationId,
		project_id: ProjectId,
		run_id: RunId,
	}
	impl TurnFixture {
		async fn start(
			path: &std::path::Path,
			settings: &[SettingKey],
		) -> Self {
			Self::with_core(path, settings, std::convert::identity).await
		}
		async fn with_core(
			path: &std::path::Path,
			settings: &[SettingKey],
			configure: impl FnOnce(Core) -> Core,
		) -> Self {
			Self::with_setup(path, settings, configure, |_| {}).await
		}
		async fn with_setup(
			path: &std::path::Path,
			settings: &[SettingKey],
			configure: impl FnOnce(Core) -> Core,
			repository: impl FnOnce(&std::path::Path),
		) -> Self {
			let (core, sender, _) =
				crate::checkpoint::tests::start_answering(path).await;
			let core = std::sync::Arc::new(configure(
				std::sync::Arc::try_unwrap(core).unwrap(),
			));
			let root = path.join("repo");
			let project_id = register_repository(&core, &root).await;
			repository(&root);
			for &key in settings {
				core.execute(
					&actor(),
					request(Command::SetSetting {
						key,
						scope: SettingScope::Project { project_id },
						value: SettingValue::Flag(true),
					}),
				)
				.await
				.unwrap();
			}
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
				panic!("conversation")
			};
			let conversation_id = conversation.conversation_id;
			let CommandOutcome::RunCreated(run) = core
				.execute(
					&actor(),
					request(Command::StartRun {
						conversation_id,
						craft: "fake".into(),
						prompt: "Edit".into(),
					}),
				)
				.await
				.unwrap()
			else {
				panic!("run")
			};
			core.perform_runs().await.unwrap();
			let fixture = Self {
				core,
				sender,
				root,
				conversation_id,
				project_id,
				run_id: run.run_id,
			};
			fixture.wait(RunLifecycle::Active).await;
			fixture
		}
		async fn wait(&self, expected: RunLifecycle) {
			tokio::time::timeout(std::time::Duration::from_secs(10), async {
				loop {
					let QueryResult::RunExecution(snapshot) = self
						.core
						.query(
							&actor(),
							Query::RunExecution {
								run_id: self.run_id,
							},
						)
						.await
						.unwrap()
					else {
						panic!("execution")
					};
					if snapshot.run.lifecycle == expected {
						break;
					}
					tokio::time::sleep(std::time::Duration::from_millis(10))
						.await;
				}
			})
			.await
			.unwrap();
		}
		async fn finish(&self, outcome: TurnOutcome) {
			self.sender
				.send(RunObservation::TurnEnded(outcome))
				.await
				.unwrap();
			self.sender
				.send(RunObservation::Ended(Some(0)))
				.await
				.unwrap();
			self.sender
				.send(RunObservation::Progress {
					offset: 2,
					checkpoint: String::new(),
				})
				.await
				.unwrap();
			self.wait(RunLifecycle::Completed).await;
		}
	}

	#[derive(Debug, Default)]
	struct GitHubPeer(std::sync::Mutex<GitHubPeerState>);
	#[derive(Debug, Default)]
	struct GitHubPeerState {
		head: String,
		draft: Option<serde_json::Value>,
		writes: Vec<&'static str>,
		lose_acknowledgement: bool,
		fail_reconciliation: bool,
		unavailable: bool,
	}
	impl GitHubHost for GitHubPeer {
		fn request<'a>(
			&'a self,
			repository: &'a str,
			request: &'a GitHubRequest,
		) -> RunFuture<'a, Result<Vec<u8>, CoreError>> {
			use serde_json::json;
			Box::pin(async move {
				assert_eq!(repository, "owner/repo");
				let mut peer = self.0.lock().unwrap();
				if peer.unavailable {
					return Err(CoreError::conflict(
						"github.test_unavailable",
						"test peer is unavailable",
					));
				}
				let response = match request {
					GitHubRequest::Repository => {
						json!({"permissions":{"push":true},"default_branch":"base"})
					}
					GitHubRequest::Branch { .. } => {
						json!({"commit":{"sha":peer.head}})
					}
					GitHubRequest::FindDrafts { .. } => peer
						.draft
						.as_ref()
						.map_or(json!([]), |draft| json!([draft])),
					GitHubRequest::ReadDraft { number } => {
						assert_eq!(*number, 7);
						peer.draft.clone().unwrap()
					}
					GitHubRequest::CreateDraft {
						title,
						body,
						head,
						base,
					} => {
						assert!(peer.draft.is_none());
						peer.writes.push("create");
						peer.draft = Some(
							json!({"number":7,"html_url":"https://github.com/owner/repo/pull/7","draft":true,"state":"open","title":title,"body":body,"head":{"ref":head,"sha":peer.head},"base":{"ref":base}}),
						);
						if std::mem::take(&mut peer.lose_acknowledgement) {
							peer.unavailable = peer.fail_reconciliation;
							return Err(CoreError::conflict(
								"github.test_ack_lost",
								"test peer lost the acknowledgement",
							));
						}
						peer.draft.clone().unwrap()
					}
					GitHubRequest::UpdateDraft {
						number,
						title,
						body,
					} => {
						assert_eq!(*number, 7);
						peer.writes.push("update");
						let draft = peer.draft.as_mut().unwrap();
						draft["title"] = json!(title);
						draft["body"] = json!(body);
						draft.clone()
					}
				};
				Ok(serde_json::to_vec(&response).unwrap())
			})
		}
	}

	#[tokio::test]
	async fn all_enabled_steps_preserve_success_and_allow_manual_draft_continuation()
	 {
		let dir = tempfile::tempdir().unwrap();
		let peer = std::sync::Arc::new(GitHubPeer::default());
		let f = TurnFixture::with_core(
			dir.path(),
			&[
				SettingKey::GitAutoBranch,
				SettingKey::GitAutoCommit,
				SettingKey::GitAutoPush,
				SettingKey::GitAutoDraftPullRequest,
			],
			|core| core.with_github_host(peer.clone()),
		)
		.await;
		let remote = dir.path().join("remote.git");
		git(&f.root, &["init", "--bare", remote.to_str().unwrap()]);
		git(
			&f.root,
			&["remote", "add", "origin", remote.to_str().unwrap()],
		);
		let before = git(&f.root, &["rev-parse", "HEAD"]);
		assert_eq!(git(&f.root, &["branch", "--show-current"]).trim(), "main");
		let hook = f.root.join(".git/hooks/pre-push");
		std::fs::write(&hook, "#!/bin/sh\nprintf 'called\\n' >> hook-calls\n")
			.unwrap();
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(
				&hook,
				std::fs::Permissions::from_mode(0o755),
			)
			.unwrap();
		}
		// Exclude the hook's evidence from the captured content.
		std::fs::write(f.root.join(".git/info/exclude"), "hook-calls\n")
			.unwrap();
		std::fs::write(f.root.join("README.md"), "Completed change\n").unwrap();
		std::fs::write(f.root.join("new.txt"), "New file\n").unwrap();
		f.finish(TurnOutcome::Completed).await;
		assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), before);
		f.core.perform_git_deliveries().await.unwrap();
		let result = deliveries(&f.core, f.conversation_id).await;
		assert_eq!(result.len(), 4);
		// The disposable Git remote reaches the hosted-provider boundary only after
		// branch, commit and push have propagated their resulting repository state.
		assert_eq!(
			result[0].outcome,
			GitDeliveryOutcome::Failed {
				code: "git.github_required".into()
			}
		);
		assert!(
			result.iter().skip(1).all(|d| matches!(
				d.outcome,
				GitDeliveryOutcome::Completed { .. }
			)),
			"{result:#?}"
		);
		let head = git(&f.root, &["rev-parse", "HEAD"]);
		assert_ne!(head, before);
		assert_eq!(git(&f.root, &["rev-parse", "main"]), before);
		assert_eq!(
			std::fs::read_to_string(f.root.join("hook-calls")).unwrap(),
			"called\n"
		);
		assert_eq!(git(&f.root, &["status", "--porcelain"]), "");
		assert_eq!(git(&f.root, &["show", "HEAD:new.txt"]), "New file\n");
		let commit = result
			.iter()
			.find(|d| d.operation == GitOperation::Commit)
			.unwrap();
		let QueryResult::Utility(job) = f
			.core
			.query(
				&actor(),
				Query::Utility {
					job_id: commit.utility_job.unwrap(),
				},
			)
			.await
			.unwrap()
		else {
			panic!("utility")
		};
		assert!(matches!(
			job.outcome,
			UtilityOutcome::Text {
				fallback_reason: Some(_),
				..
			}
		));
		let branch = format!("refs/heads/jet/{}", f.conversation_id.0);
		assert_eq!(
			git(&f.root, &["ls-remote", "origin", &branch])
				.split_whitespace()
				.next(),
			Some(head.trim())
		);
		f.core.perform_git_deliveries().await.unwrap();
		assert_eq!(deliveries(&f.core, f.conversation_id).await, result);
		assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);

		git(
			&f.root,
			&[
				"remote",
				"set-url",
				"origin",
				"https://github.com/owner/repo.git",
			],
		);
		peer.0.lock().unwrap().head = head.trim().into();
		f.core
			.execute(
				&actor(),
				request(Command::DeliverGit {
					conversation_id: f.conversation_id,
					checkpoint: Some(GitCheckpoint {
						run_id: f.run_id,
						turn: 1,
					}),
					operation: GitOperation::DraftPullRequest {
						remote: "origin".into(),
						base: None,
					},
				}),
			)
			.await
			.unwrap();
		f.core.perform_git_deliveries().await.unwrap();
		assert!(matches!(
			deliveries(&f.core, f.conversation_id).await[0].outcome,
			GitDeliveryOutcome::Completed {
				pull_request: Some(_),
				..
			}
		));
		assert_eq!(peer.0.lock().unwrap().writes, vec!["create"]);
		assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
		f.core.close().await;
	}

	#[tokio::test]
	async fn failed_turn_does_not_automate_and_policy_revocation_stops_admitted_effects()
	 {
		for successful in [false, true] {
			let dir = tempfile::tempdir().unwrap();
			let f =
				TurnFixture::start(dir.path(), &[SettingKey::GitAutoCommit])
					.await;
			let head = git(&f.root, &["rev-parse", "HEAD"]);
			std::fs::write(f.root.join("README.md"), "Uncommitted change\n")
				.unwrap();
			f.finish(if successful {
				TurnOutcome::Completed
			} else {
				TurnOutcome::Interrupted
			})
			.await;
			if successful {
				f.core
					.execute(
						&actor(),
						request(Command::SetSetting {
							key: SettingKey::GitAutoCommit,
							scope: SettingScope::Conversation {
								conversation_id: f.conversation_id,
							},
							value: SettingValue::Flag(false),
						}),
					)
					.await
					.unwrap();
			}
			f.core.perform_git_deliveries().await.unwrap();
			let result = deliveries(&f.core, f.conversation_id).await;
			let outcomes: Vec<_> =
				result.into_iter().map(|d| d.outcome).collect();
			assert_eq!(
				outcomes,
				if successful {
					vec![GitDeliveryOutcome::Failed {
						code: "git.policy_changed".into(),
					}]
				} else {
					vec![]
				}
			);
			assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
		}
	}

	#[tokio::test]
	async fn later_edits_are_never_swept_into_an_automatic_commit() {
		let dir = tempfile::tempdir().unwrap();
		let f =
			TurnFixture::start(dir.path(), &[SettingKey::GitAutoCommit]).await;
		let head = git(&f.root, &["rev-parse", "HEAD"]);
		std::fs::write(f.root.join("README.md"), "Captured change\n").unwrap();
		f.finish(TurnOutcome::Completed).await;
		std::fs::write(f.root.join("README.md"), "Later user edit\n").unwrap();
		f.core.perform_git_deliveries().await.unwrap();
		assert_eq!(
			deliveries(&f.core, f.conversation_id).await[0].outcome,
			GitDeliveryOutcome::Failed {
				code: "git.content_changed".into()
			}
		);
		assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
		assert_eq!(
			std::fs::read_to_string(f.root.join("README.md")).unwrap(),
			"Later user edit\n"
		);
	}

	#[tokio::test]
	async fn shared_checkout_blocks_delivery_during_another_run_and_runs_during_delivery()
	 {
		let dir = tempfile::tempdir().unwrap();
		let f =
			TurnFixture::start(dir.path(), &[SettingKey::GitAutoCommit]).await;
		let CommandOutcome::ConversationCreated(other) = f
			.core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::LocalCheckout {
						project_id: f.project_id,
					},
				}),
			)
			.await
			.unwrap()
		else {
			panic!("conversation")
		};
		let branch = Command::DeliverGit {
			conversation_id: other.conversation_id,
			checkpoint: None,
			operation: GitOperation::Branch {
				name: "other/branch".into(),
			},
		};
		assert_eq!(
			f.core
				.execute(&actor(), request(branch.clone()))
				.await
				.unwrap_err()
				.code,
			"git.run_active"
		);
		std::fs::write(f.root.join("README.md"), "Completed content\n")
			.unwrap();
		f.finish(TurnOutcome::Completed).await;
		assert_eq!(
			f.core
				.execute(&actor(), request(branch))
				.await
				.unwrap_err()
				.code,
			"git.delivery_unresolved"
		);
		assert_eq!(
			f.core
				.execute(
					&actor(),
					request(Command::StartRun {
						conversation_id: other.conversation_id,
						craft: "fake".into(),
						prompt: "next".into()
					})
				)
				.await
				.unwrap_err()
				.code,
			"git.delivery_unresolved"
		);
		f.core.perform_git_deliveries().await.unwrap();
		assert!(matches!(
			deliveries(&f.core, f.conversation_id).await[0].outcome,
			GitDeliveryOutcome::Completed { .. }
		));
	}

	#[tokio::test]
	async fn committing_sparse_checkout_preserves_excluded_paths_and_index_flags()
	 {
		let dir = tempfile::tempdir().unwrap();
		let f = TurnFixture::with_setup(
			dir.path(),
			&[SettingKey::GitAutoCommit],
			std::convert::identity,
			|root| {
				std::fs::create_dir(root.join("one")).unwrap();
				std::fs::create_dir(root.join("two")).unwrap();
				std::fs::write(root.join("one/a"), "visible\n").unwrap();
				std::fs::write(root.join("two/b"), "excluded\n").unwrap();
				git(root, &["add", "."]);
				git(root, &["commit", "-m", "Added sparse fixture"]);
				git(root, &["sparse-checkout", "set", "--cone", "one"]);
			},
		)
		.await;
		assert!(!f.root.join("two/b").exists());
		std::fs::write(f.root.join("one/a"), "changed\n").unwrap();
		f.finish(TurnOutcome::Completed).await;
		f.core.perform_git_deliveries().await.unwrap();
		let result = deliveries(&f.core, f.conversation_id).await;
		assert!(
			matches!(result[0].outcome, GitDeliveryOutcome::Completed { .. }),
			"{result:#?}"
		);
		assert_eq!(git(&f.root, &["ls-files", "-t", "two/b"]), "S two/b\n");
		assert_eq!(git(&f.root, &["status", "--porcelain"]), "");
		assert_eq!(git(&f.root, &["show", "HEAD:two/b"]), "excluded\n");
		assert!(!f.root.join("two/b").exists());
	}

	#[tokio::test]
	async fn delivery_waits_for_the_parser_checkpoint_before_mutating_a_live_run()
	 {
		let dir = tempfile::tempdir().unwrap();
		let f = TurnFixture::start(
			dir.path(),
			&[SettingKey::GitAutoBranch, SettingKey::GitAutoCommit],
		)
		.await;
		let before = git(&f.root, &["rev-parse", "HEAD"]);
		std::fs::write(
			f.root.join("README.md"),
			"Completed turn awaiting parser checkpoint\n",
		)
		.unwrap();
		f.sender
			.send(RunObservation::TurnEnded(TurnOutcome::Completed))
			.await
			.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			while deliveries(&f.core, f.conversation_id).await.is_empty() {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		f.core.perform_git_deliveries().await.unwrap();
		assert_eq!(
			deliveries(&f.core, f.conversation_id)
				.await
				.into_iter()
				.map(|d| d.outcome)
				.collect::<Vec<_>>(),
			vec![GitDeliveryOutcome::Pending, GitDeliveryOutcome::Pending]
		);
		assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), before);
		f.sender.send(RunObservation::Ended(Some(0))).await.unwrap();
		f.wait(RunLifecycle::Completed).await;
		f.core.perform_git_deliveries().await.unwrap();
		assert_eq!(
			deliveries(&f.core, f.conversation_id)
				.await
				.into_iter()
				.map(|d| d.outcome)
				.collect::<Vec<_>>(),
			vec![GitDeliveryOutcome::Pending, GitDeliveryOutcome::Pending]
		);
		assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), before);

		f.sender
			.send(RunObservation::Progress {
				offset: 2,
				checkpoint: String::new(),
			})
			.await
			.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				f.core.perform_git_deliveries().await.unwrap();
				if deliveries(&f.core, f.conversation_id).await.iter().all(
					|d| {
						matches!(
							d.outcome,
							GitDeliveryOutcome::Completed { .. }
						)
					},
				) {
					break;
				}
				f.core.wait_for_utility_work().await;
			}
		})
		.await
		.unwrap();
		assert_ne!(git(&f.root, &["rev-parse", "HEAD"]), before);
		f.core.close().await;
	}

	mod github_tests {
		use super::*;
		use pretty_assertions::assert_eq;

		#[tokio::test]
		async fn github_recovers_a_lost_ack_and_updates_the_same_conversation_draft_after_restart()
		 {
			let dir = tempfile::tempdir().unwrap();
			let peer = std::sync::Arc::new(GitHubPeer::default());
			let f = TurnFixture::with_core(
				dir.path(),
				&[SettingKey::GitAutoDraftPullRequest],
				|core| core.with_github_host(peer.clone()),
			)
			.await;
			git(
				&f.root,
				&[
					"remote",
					"add",
					"origin",
					"https://github.com/owner/repo.git",
				],
			);
			{
				let mut state = peer.0.lock().unwrap();
				state.head = git(&f.root, &["rev-parse", "HEAD"]).trim().into();
				state.lose_acknowledgement = true;
			}
			f.finish(TurnOutcome::Completed).await;
			f.core.perform_git_deliveries().await.unwrap();
			let result = deliveries(&f.core, f.conversation_id).await;
			assert!(
				matches!(&result[0].outcome, GitDeliveryOutcome::Completed { pull_request: Some(url), .. } if url == "https://github.com/owner/repo/pull/7"),
				"{result:#?}"
			);
			f.core.close().await;
			let core = start_core(&dir.path().join("plane.sqlite3"))
				.await
				.with_github_host(peer.clone());
			core.execute(
				&actor(),
				request(Command::DeliverGit {
					conversation_id: f.conversation_id,
					checkpoint: Some(GitCheckpoint {
						run_id: f.run_id,
						turn: 1,
					}),
					operation: GitOperation::DraftPullRequest {
						remote: "origin".into(),
						base: None,
					},
				}),
			)
			.await
			.unwrap();
			core.perform_git_deliveries().await.unwrap();
			assert_eq!(peer.0.lock().unwrap().writes, vec!["create", "update"]);
			assert!(deliveries(&core, f.conversation_id).await.iter().all(
				|d| matches!(d.outcome, GitDeliveryOutcome::Completed { .. })
			));
			// A user publishing that draft removes it from Jet's automatic authority.
			peer.0.lock().unwrap().draft.as_mut().unwrap()["draft"] =
				serde_json::json!(false);
			core.execute(
				&actor(),
				request(Command::DeliverGit {
					conversation_id: f.conversation_id,
					checkpoint: Some(GitCheckpoint {
						run_id: f.run_id,
						turn: 1,
					}),
					operation: GitOperation::DraftPullRequest {
						remote: "origin".into(),
						base: None,
					},
				}),
			)
			.await
			.unwrap();
			core.perform_git_deliveries().await.unwrap();
			assert_eq!(
				deliveries(&core, f.conversation_id).await[0].outcome,
				GitDeliveryOutcome::Failed {
					code: "git.draft_conflict".into()
				}
			);
			assert_eq!(peer.0.lock().unwrap().writes, vec!["create", "update"]);
		}

		#[tokio::test]
		async fn uncertain_delivery_waits_for_user_acknowledgement_and_is_never_retried()
		 {
			let dir = tempfile::tempdir().unwrap();
			let peer = std::sync::Arc::new(GitHubPeer::default());
			let f = TurnFixture::with_core(
				dir.path(),
				&[SettingKey::GitAutoDraftPullRequest],
				|core| core.with_github_host(peer.clone()),
			)
			.await;
			git(
				&f.root,
				&[
					"remote",
					"add",
					"origin",
					"https://github.com/owner/repo.git",
				],
			);
			{
				let mut state = peer.0.lock().unwrap();
				state.head = git(&f.root, &["rev-parse", "HEAD"]).trim().into();
				state.lose_acknowledgement = true;
				state.fail_reconciliation = true;
			}
			f.finish(TurnOutcome::Completed).await;
			f.core.perform_git_deliveries().await.unwrap();
			let before = deliveries(&f.core, f.conversation_id).await.remove(0);
			assert_eq!(before.outcome, GitDeliveryOutcome::OutcomeUnknown);
			let branch = Command::DeliverGit {
				conversation_id: f.conversation_id,
				checkpoint: None,
				operation: GitOperation::Branch {
					name: "user/reviewed".into(),
				},
			};
			assert_eq!(
				f.core
					.execute(&actor(), request(branch.clone()))
					.await
					.unwrap_err()
					.code,
				"git.delivery_unresolved"
			);
			let command = request(Command::AcknowledgeGitDelivery {
				delivery_id: before.delivery_id,
			});
			let receipt =
				f.core.execute(&actor(), command.clone()).await.unwrap();
			assert_eq!(
				receipt,
				CommandOutcome::GitDeliveryAcknowledged {
					delivery_id: before.delivery_id
				}
			);
			f.core.execute(&actor(), request(branch)).await.unwrap();
			f.core.perform_git_deliveries().await.unwrap();
			let after = deliveries(&f.core, f.conversation_id)
				.await
				.into_iter()
				.find(|d| d.delivery_id == before.delivery_id)
				.unwrap();
			assert_eq!(
				after,
				GitDelivery {
					acknowledged_by: Some(actor().client_id()),
					..before
				}
			);
			assert_eq!(
				f.core.execute(&actor(), command).await.unwrap(),
				receipt
			);
			assert_eq!(peer.0.lock().unwrap().writes, vec!["create"]);
		}

		#[tokio::test]
		async fn manual_draft_can_continue_after_committing_the_retained_checkpoint()
		 {
			let dir = tempfile::tempdir().unwrap();
			let peer = std::sync::Arc::new(GitHubPeer::default());
			let f = TurnFixture::with_core(dir.path(), &[], |core| {
				core.with_github_host(peer.clone())
			})
			.await;
			git(
				&f.root,
				&[
					"remote",
					"add",
					"origin",
					"https://github.com/owner/repo.git",
				],
			);
			std::fs::write(f.root.join("README.md"), "Manual commit content\n")
				.unwrap();
			f.finish(TurnOutcome::Completed).await;
			let checkpoint = Some(GitCheckpoint {
				run_id: f.run_id,
				turn: 1,
			});
			f.core
				.execute(
					&actor(),
					request(Command::DeliverGit {
						conversation_id: f.conversation_id,
						checkpoint,
						operation: GitOperation::Commit,
					}),
				)
				.await
				.unwrap();
			f.core.perform_git_deliveries().await.unwrap();
			assert!(matches!(
				deliveries(&f.core, f.conversation_id).await[0].outcome,
				GitDeliveryOutcome::Completed { .. }
			));
			peer.0.lock().unwrap().head =
				git(&f.root, &["rev-parse", "HEAD"]).trim().into();
			f.core
				.execute(
					&actor(),
					request(Command::DeliverGit {
						conversation_id: f.conversation_id,
						checkpoint,
						operation: GitOperation::DraftPullRequest {
							remote: "origin".into(),
							base: None,
						},
					}),
				)
				.await
				.unwrap();
			f.core.perform_git_deliveries().await.unwrap();
			let result = deliveries(&f.core, f.conversation_id).await;
			assert!(
				result.iter().all(|d| matches!(
					d.outcome,
					GitDeliveryOutcome::Completed { .. }
				)),
				"{result:#?}"
			);
			assert_eq!(peer.0.lock().unwrap().writes, vec!["create"]);
		}
	}

	mod disk_pressure {
		use super::*;
		use crate::run::state::Observation;
		use pretty_assertions::assert_eq;

		#[tokio::test]
		async fn recovered_space_cannot_make_an_uncaptured_boundary_safe_for_delivery()
		 {
			let dir = tempfile::tempdir().unwrap();
			let f = TurnFixture::start(dir.path(), &[]).await;
			let head = git(&f.root, &["rev-parse", "HEAD"]);
			std::fs::write(
				f.root.join("README.md"),
				"Pre-existing user edit\n",
			)
			.unwrap();
			// Record real turn boundaries through a Core whose reserve cannot be met.
			let pressured = start_core(&dir.path().join("plane.sqlite3"))
				.await
				.with_artifact_limits(ArtifactLimits {
					free_reserve_bytes: u64::MAX,
					..Default::default()
				});
			pressured
				.observe_run(
					f.run_id,
					Observation::TurnEnded(TurnOutcome::Completed),
				)
				.await
				.unwrap();
			pressured
				.observe_run(f.run_id, Observation::TurnStarted)
				.await
				.unwrap();
			f.core
				.execute(
					&actor(),
					request(Command::SetSetting {
						key: SettingKey::GitAutoCommit,
						scope: SettingScope::Project {
							project_id: f.project_id,
						},
						value: SettingValue::Flag(true),
					}),
				)
				.await
				.unwrap();
			std::fs::write(
				f.root.join("agent.txt"),
				"Agent edit after recovery\n",
			)
			.unwrap();
			f.finish(TurnOutcome::Completed).await;
			for scope in [
				DiffScope::Turn { turn: 2 },
				DiffScope::Historical {
					from_turn: 1,
					to_turn: 2,
				},
			] {
				let QueryResult::ChangeDiff(diff) = f
					.core
					.query(
						&actor(),
						Query::ChangeDiff {
							run_id: f.run_id,
							scope,
						},
					)
					.await
					.unwrap()
				else {
					panic!("Diff")
				};
				assert_eq!(
					(
						diff.before.uncommitted.availability,
						diff.after.uncommitted.availability,
						diff.artifact.availability,
						diff.patch,
						diff.patch_truncated
					),
					(
						ArtifactAvailability::DiskPressure,
						ArtifactAvailability::Stored,
						ArtifactAvailability::DiskPressure,
						String::new(),
						true
					)
				);
			}
			f.core.perform_git_deliveries().await.unwrap();
			let outcomes: Vec<_> = deliveries(&f.core, f.conversation_id)
				.await
				.into_iter()
				.map(|delivery| delivery.outcome)
				.collect();
			assert_eq!(
				outcomes,
				vec![GitDeliveryOutcome::Failed {
					code: "git.incomplete_or_dirty_baseline".into(),
				}]
			);
			assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
			assert_eq!(
				std::fs::read_to_string(f.root.join("README.md")).unwrap(),
				"Pre-existing user edit\n"
			);
		}
	}
}
