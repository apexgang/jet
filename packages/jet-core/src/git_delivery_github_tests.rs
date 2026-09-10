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
	assert!(
		deliveries(&core, f.conversation_id)
			.await
			.iter()
			.all(|d| matches!(d.outcome, GitDeliveryOutcome::Completed { .. }))
	);
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
	let receipt = f.core.execute(&actor(), command.clone()).await.unwrap();
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
	assert_eq!(f.core.execute(&actor(), command).await.unwrap(), receipt);
	assert_eq!(peer.0.lock().unwrap().writes, vec!["create"]);
}

#[tokio::test]
async fn manual_draft_can_continue_after_committing_the_retained_checkpoint() {
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
		result
			.iter()
			.all(|d| matches!(d.outcome, GitDeliveryOutcome::Completed { .. })),
		"{result:#?}"
	);
	assert_eq!(peer.0.lock().unwrap().writes, vec!["create"]);
}
