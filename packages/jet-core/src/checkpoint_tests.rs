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
				working_tree: WorkingTreeRequest::LocalCheckout { project_id },
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
	assert!(diff.patch.contains("-# Jet\n+Turn one\n"), "{}", diff.patch);
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
	let original_head = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
	std::fs::write(root.join("README.md"), "Initial dirty content\n").unwrap();
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
	let first = wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
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
				.unwrap() && state.activity == Some(RunActivity::WaitingForUser)
			{
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
	git(&root, &["reset", "--hard", &original_head]);
	std::fs::write(root.join("README.md"), "Partial second turn\n").unwrap();
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
	for path in ["user.txt", "terminal.txt", "harness.txt", "unexplained.txt"] {
		std::fs::write(root.join(path), "Before\n").unwrap();
	}
	git(&root, &["add", "-A"]);
	git(&root, &["commit", "-qm", "Base"]);
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
			before_object: git(&root, &["rev-parse", &format!("HEAD:{path}")])
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
	let diff = wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
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
	let diff = wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
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
			let state: crate::run_state::State =
				crate::run_state::decode(&execution.state)?;
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
async fn a_direct_edit_intent_finishes_after_restart_before_receipt_replay() {
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
	let envelope = crate::test_support::request_with_id(command_id, command);
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
	wait_for(&core, run.run_id, RunLifecycle::Active).await;
	for i in 0..300 {
		std::fs::write(root.join(format!("file-{i:03}.txt")), "new").unwrap();
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
	let diff = wait_diff(&core, run.run_id, DiffScope::Turn { turn: 1 }).await;
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
	let (sender, receiver) = mpsc::channel(32);
	let core = crate::test_support::start_core(&home.join("plane.sqlite3"))
		.await
		.with_run_host(Arc::new(Host(Mutex::new(Some(receiver)))));
	(Arc::new(core), sender)
}

#[derive(Debug)]
struct Host(Mutex<Option<mpsc::Receiver<RunObservation>>>);
impl RunHost for Host {
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
			}) as Box<dyn RunConnection>)
		})
	}
}
struct Connection {
	receiver: Mutex<mpsc::Receiver<RunObservation>>,
	started: std::sync::atomic::AtomicBool,
}
impl RunConnection for Connection {
	fn submit_turn(
		&self,
		_turn_id: uuid::Uuid,
		_prompt: String,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async { panic!("no input queued in this fixture") })
	}
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the shared fixture port has one serialized observation receiver"
	)]
	fn receive(&self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
		Box::pin(async move {
			if !self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
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

#[path = "checkpoint_regression_tests.rs"]
mod regression;
