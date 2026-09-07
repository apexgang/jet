use super::*;
use pretty_assertions::assert_eq;

async fn active_run(core: &Arc<Core>, root: &Path) -> RunId {
	let project_id = register_repository(core, root).await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::LocalCheckout { project_id },
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
	let before = git(root, &["hash-object", "README.md"]).trim().to_owned();
	std::fs::write(root.join("README.md"), text).unwrap();
	ChangeEvidence {
		activity_id: uuid::Uuid::new_v4().to_string(),
		origin: ChangeOrigin::Harness { run_id },
		path: "README.md".into(),
		before_object: before,
		after_object: git(root, &["hash-object", "README.md"]).trim().into(),
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
		assert_eq!(diff.files[0].origin, ChangeOrigin::ExternalOrUnknown);
		assert!(!diff.patch.is_empty());
	}
}

#[tokio::test]
async fn aggregate_scopes_cannot_upgrade_unknown_intermediate_changes() {
	let dir = tempfile::tempdir().unwrap();
	let (core, sender) = start(dir.path()).await;
	let root = dir.path().join("repo");
	let run_id = active_run(&core, &root).await;
	core.record_change_evidence(run_id, edit(&root, run_id, "Harness B\n"))
		.await
		.unwrap();
	std::fs::write(root.join("README.md"), "External C\n").unwrap();
	sender
		.send(RunObservation::TurnEnded(TurnOutcome::Completed))
		.await
		.unwrap();
	progress(&sender, 2).await;
	let first = wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
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
				.unwrap() && state.activity == Some(RunActivity::WaitingForUser)
			{
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
	std::fs::write(root.join("README.md"), "Harness B\n").unwrap();
	let current = wait_diff(&core, run_id, DiffScope::Current).await;
	assert_eq!(current.files[0].origin, ChangeOrigin::ExternalOrUnknown);
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
		assert_eq!(diff.files[0].origin, ChangeOrigin::ExternalOrUnknown);
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
	assert_eq!(wait_diff(&core, run_id, DiffScope::Current).await, first);
	assert_eq!(std::fs::read(&budget).unwrap(), reserved);
	// Seed a durable almost-exhausted reservation, as after earlier large
	// ingestions. This exercises the real query without allocating 2 GiB.
	std::fs::write(&budget, (2_u64 * 1024 * 1024 * 1024 - 1).to_be_bytes())
		.unwrap();
	std::fs::write(root.join("README.md"), "Another distinct patch\n").unwrap();
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
	let completed = wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
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
	let root = dir.path().join("repo");
	let run_id = active_run(&core, &root).await;
	let size = 513_u64 * 1024 * 1024;
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
	let diff = wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
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
		!git(&root, &["ls-tree", "-r", &diff.after.tree]).contains("large.bin")
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
	let first = wait_diff(&core, run_id, DiffScope::Turn { turn: 1 }).await;
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
		.send(RunObservation::NativeConversation("second-turn".into()))
		.await
		.unwrap();
	progress(&sender, 3).await;
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			if let QueryResult::RunExecution(state) = core
				.query(&actor(), Query::RunExecution { run_id })
				.await
				.unwrap() && state.native_conversation.as_deref()
				== Some("second-turn")
			{
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
	std::fs::write(root.join("README.md"), "Small again\n").unwrap();
	let after = git(&root, &["hash-object", "README.md"]).trim().to_owned();
	sender
		.send(RunObservation::Completed("second-turn".into()))
		.await
		.unwrap();
	sender.send(RunObservation::Ended(Some(0))).await.unwrap();
	progress(&sender, 4).await;
	wait_for(&core, run_id, RunLifecycle::Completed).await;
	let second = wait_diff(&core, run_id, DiffScope::Turn { turn: 2 }).await;
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
