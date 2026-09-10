use super::{install_craft, start_core};
use crate::test_support::{
	actor, conversation_snapshot, register_repository, request,
};
use crate::{
	ArtifactLimits, Command, CommandOutcome, RetentionPolicy,
	WorkingTreeRequest,
};
use pretty_assertions::assert_eq;

#[tokio::test]
async fn pressure_defers_an_admitted_launch_without_failing_or_replacing_the_run()
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
	core.execute(
		&actor(),
		request(Command::StartRun {
			conversation_id: conversation.conversation_id,
			craft: "fake".into(),
			prompt: "Continue".into(),
		}),
	)
	.await
	.unwrap();
	let before =
		conversation_snapshot(&core, conversation.conversation_id).await;
	// A launch that would fail after Craft removal must remain pending under pressure.
	std::fs::write(dir.path().join("crafts/fake-craft"), b"changed").unwrap();
	let core = std::sync::Arc::new(core.with_artifact_limits(ArtifactLimits {
		free_reserve_bytes: u64::MAX,
		..Default::default()
	}));
	core.perform_runs().await.unwrap();
	assert_eq!(
		conversation_snapshot(&core, conversation.conversation_id).await,
		before
	);
	let core = std::sync::Arc::new(
		std::sync::Arc::try_unwrap(core)
			.unwrap()
			.with_artifact_limits(ArtifactLimits::default()),
	);
	core.perform_runs().await.unwrap();
	assert_eq!(
		conversation_snapshot(&core, conversation.conversation_id)
			.await
			.runs[0]
			.lifecycle,
		crate::RunLifecycle::Failed
	);
}
