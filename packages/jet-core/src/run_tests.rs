use pretty_assertions::assert_eq;

use crate::test_support::{
	actor, conversation_snapshot, events, register_repository, request,
};
use crate::{Command, CommandOutcome, RetentionPolicy, WorkingTreeRequest};

#[tokio::test]
async fn starting_a_run_requires_a_project_without_recording_work() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("plane.sqlite3")).await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Conversation")
	};
	let before = events(&core).await;
	let error = core
		.execute(
			&actor(),
			request(Command::StartRun {
				conversation_id: conversation.conversation_id,
				craft: "fake".into(),
				prompt: "Make a change".into(),
			}),
		)
		.await
		.unwrap_err();
	assert_eq!(error.code, "run.project_required");
	assert_eq!(
		(
			conversation_snapshot(&core, conversation.conversation_id)
				.await
				.runs,
			events(&core).await
		),
		(vec![], before)
	);
}

#[tokio::test]
async fn admitted_run_and_exact_retry_survive_restart() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let core = start_core(&path).await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	install_craft(dir.path());
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
	let envelope = request(Command::StartRun {
		conversation_id: conversation.conversation_id,
		craft: "fake".into(),
		prompt: "Make a change".into(),
	});
	let outcome = core.execute(&actor(), envelope.clone()).await.unwrap();
	let CommandOutcome::RunCreated(run) = &outcome else {
		panic!("Run")
	};
	assert_eq!(run.lifecycle, crate::RunLifecycle::Starting);
	let snapshot =
		conversation_snapshot(&core, conversation.conversation_id).await;
	let journal = events(&core).await;
	core.close().await;
	let core = start_core(&path).await;
	assert_eq!(core.execute(&actor(), envelope).await.unwrap(), outcome);
	assert_eq!(
		(
			conversation_snapshot(&core, conversation.conversation_id).await,
			events(&core).await
		),
		(snapshot, journal)
	);
}

fn install_craft(home: &std::path::Path) {
	use sha2::{Digest, Sha256};
	let executable = std::path::Path::new("/bin/cat").canonicalize().unwrap();
	std::fs::create_dir_all(home.join("crafts")).unwrap();
	let installed = home.join("crafts/fake-craft");
	std::fs::copy(executable, &installed).unwrap();
	let executable = installed.canonicalize().unwrap();
	let digest =
		format!("{:x}", Sha256::digest(std::fs::read(&executable).unwrap()));
	let installation = serde_json::json!({
		"executable": executable, "sha256": digest,
		"specification": {
			"schema": {"major":1,"minor":0}, "id":"fake", "harness":"fake",
			"protocol":{"family":"craft","versions":[{"major":1,"minor":1}],"capabilities":["runs"]},
			"features":[{"name":"turns"}], "broker_permissions":[], "host_access":[]
		}
	});
	std::fs::create_dir_all(home.join("crafts")).unwrap();
	std::fs::write(home.join("crafts/fake.json"), installation.to_string())
		.unwrap();
}

#[tokio::test]
async fn managed_run_cannot_be_completed_by_a_client_while_its_process_may_live()
 {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("plane.sqlite3")).await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	install_craft(dir.path());
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
				prompt: "Make a change".into(),
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Run")
	};
	let before =
		conversation_snapshot(&core, conversation.conversation_id).await;
	let error = core
		.execute(
			&actor(),
			request(Command::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: crate::RunLifecycle::Failed,
			}),
		)
		.await
		.unwrap_err();
	assert_eq!(error.code, "run.managed_lifecycle");
	assert_eq!(
		conversation_snapshot(&core, conversation.conversation_id).await,
		before
	);
}

#[tokio::test]
async fn an_unavailable_pinned_craft_fails_before_launch_and_releases_the_run()
{
	let dir = tempfile::tempdir().unwrap();
	let core = std::sync::Arc::new(
		start_core(&dir.path().join("plane.sqlite3")).await,
	);
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	install_craft(dir.path());
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
				prompt: "Make a change".into(),
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Run")
	};
	std::fs::write(
		dir.path().join("crafts/fake-craft"),
		"changed after admission",
	)
	.unwrap();
	core.perform_runs().await.unwrap();
	let crate::QueryResult::RunExecution(execution) = core
		.query(&actor(), crate::Query::RunExecution { run_id: run.run_id })
		.await
		.unwrap()
	else {
		panic!("Execution")
	};
	assert_eq!(
		(
			execution.run.lifecycle,
			execution.activity,
			execution.processes
		),
		(crate::RunLifecycle::Failed, None, vec![])
	);
	core.execute(
		&actor(),
		request(Command::CreateRun {
			conversation_id: conversation.conversation_id,
		}),
	)
	.await
	.unwrap();
}

// Only the host's installed-Craft lookup is supplied here. Tests continue to
// observe admission and reconciliation exclusively through Commands/Queries/Events.
async fn start_core(path: &std::path::Path) -> crate::Core {
	crate::test_support::start_core(path)
		.await
		.with_run_host(std::sync::Arc::new(AdmissionHost))
}
#[derive(Debug)]
struct AdmissionHost;
impl crate::RunHost for AdmissionHost {
	fn pin(
		&self,
		home: std::path::PathBuf,
		_id: String,
	) -> crate::RunFuture<'_, Result<crate::PinnedCraft, crate::CoreError>> {
		Box::pin(async move {
			let value: serde_json::Value = serde_json::from_slice(
				&std::fs::read(home.join("crafts/fake.json")).unwrap(),
			)
			.unwrap();
			Ok(crate::PinnedCraft {
				executable: value["executable"].as_str().unwrap().into(),
				sha256: value["sha256"].as_str().unwrap().into(),
				adapter_state: "fixture-v1".into(),
			})
		})
	}
	fn start(
		&self,
		_home: std::path::PathBuf,
		_run_id: crate::RunId,
		_plan: crate::LaunchPlan,
	) -> crate::RunFuture<
		'_,
		Result<Box<dyn crate::RunConnection>, crate::RunStartError>,
	> {
		Box::pin(async {
			panic!("changed artifact must fail before host launch")
		})
	}
}
