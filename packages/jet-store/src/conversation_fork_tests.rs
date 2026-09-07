use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::{
	ConversationOriginRecord, ForkLaunchContextRecord, NewConversation,
	RetentionPolicy, Store, WorkingTreeRecord,
};

#[tokio::test]
async fn fork_launch_context_needs_no_live_source_rows() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let conversation_id = Uuid::now_v7();
	let launch = ForkLaunchContextRecord {
		conversation_id,
		checkpoint_commit: "a".repeat(40),
		checkpoint_tree: "b".repeat(64),
		source_craft: Some(r#"{"version":1}"#.into()),
		source_native_conversation: Some("native-source".into()),
		context_json: r#"{"entries":[],"history_truncated":false}"#.into(),
	};
	store
		.write(async |tx| {
			tx.insert_conversation(NewConversation {
				conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRecord::NoProject,
				origin: ConversationOriginRecord::Forked {
					source_conversation_id: Uuid::now_v7(),
					source_run_id: Uuid::now_v7(),
					checkpoint_turn: 1,
				},
				created_at_unix_ms: 1,
			})
			.await?;
			tx.insert_conversation_fork_launch(&launch).await
		})
		.await
		.unwrap();

	let stored = store
		.read(async |tx| tx.conversation_fork_launch(conversation_id).await)
		.await
		.unwrap();

	assert_eq!(stored, Some(launch));
}
