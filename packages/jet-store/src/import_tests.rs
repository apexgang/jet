use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{ImportedConversationRecord, NewImportedConversation};
use crate::{
	ActorRecord, ConversationOriginRecord, NewConversation, RetentionPolicy,
	Store, StoreError, WorkingTreeRecord,
};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

fn import(harness: &str, native_conversation: &str) -> NewImportedConversation {
	NewImportedConversation {
		import_id: Uuid::now_v7(),
		harness: harness.into(),
		native_conversation: native_conversation.into(),
		working_directory: Some("/home/jet/repo".into()),
		imported_by: ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		},
		imported_at_unix_ms: NOW_UNIX_MS,
	}
}

fn recorded(
	import: &NewImportedConversation,
	resumed_as: Option<Uuid>,
) -> ImportedConversationRecord {
	ImportedConversationRecord {
		import_id: import.import_id,
		harness: import.harness.clone(),
		native_conversation: import.native_conversation.clone(),
		working_directory: import.working_directory.clone(),
		imported_by: import.imported_by,
		imported_at_unix_ms: import.imported_at_unix_ms,
		resumed_as,
	}
}

fn continuing(import_id: Uuid) -> NewConversation {
	NewConversation {
		conversation_id: Uuid::now_v7(),
		retention: RetentionPolicy::Retain,
		working_tree: WorkingTreeRecord::NoProject,
		origin: ConversationOriginRecord::Imported { import_id },
		created_at_unix_ms: NOW_UNIX_MS,
	}
}

/// An import outlives the daemon that registered it, is found by its
/// identity or its Harness-native identity, and reports the Conversation
/// that continues it once one exists (ADR-0010).
#[tokio::test]
async fn imports_survive_reopening_and_report_their_continuation() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let resumed = import("codex", "thread-1");
	let waiting = NewImportedConversation {
		working_directory: None,
		..import("claude-code", "session-2")
	};
	let continuation = continuing(resumed.import_id);

	let first = Store::open(&path).await.unwrap();
	first
		.write(async |tx| {
			tx.insert_imported_conversation(resumed.clone()).await?;
			tx.insert_imported_conversation(waiting.clone()).await?;
			tx.insert_conversation(continuation).await
		})
		.await
		.unwrap();
	first.close().await;

	let second = Store::open(&path).await.unwrap();
	let (listed, by_id, by_identity, unknown, conversation) = second
		.read(async |tx| {
			Ok::<_, StoreError>((
				tx.imported_conversations().await?,
				tx.imported_conversation(waiting.import_id).await?,
				tx.imported_conversation_by_identity("codex", "thread-1")
					.await?,
				tx.imported_conversation_by_identity("codex", "thread-9")
					.await?,
				tx.conversation(continuation.conversation_id).await?,
			))
		})
		.await
		.unwrap();

	assert_eq!(
		(
			listed,
			by_id,
			by_identity,
			unknown,
			conversation.map(|c| c.origin)
		),
		(
			vec![
				recorded(&resumed, Some(continuation.conversation_id)),
				recorded(&waiting, None),
			],
			Some(recorded(&waiting, None)),
			Some(recorded(&resumed, Some(continuation.conversation_id))),
			None,
			Some(ConversationOriginRecord::Imported {
				import_id: resumed.import_id,
			}),
		)
	);
}

/// One Harness-native identity is one import, and one import is continued
/// by one Conversation. The core checks before it inserts; the schema
/// refuses either second row even so.
#[tokio::test]
async fn an_identity_is_imported_once_and_continued_once() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let first = import("codex", "thread-1");
	let again = import("codex", "thread-1");
	let elsewhere = import("claude-code", "thread-1");
	let continuation = continuing(first.import_id);

	store
		.write(async |tx| {
			tx.insert_imported_conversation(first.clone()).await?;
			tx.insert_imported_conversation(elsewhere.clone()).await?;
			tx.insert_conversation(continuation).await
		})
		.await
		.unwrap();
	let refused_import = store
		.write(async |tx| tx.insert_imported_conversation(again).await)
		.await
		.unwrap_err();
	let refused_continuation = store
		.write(async |tx| {
			tx.insert_conversation(continuing(first.import_id)).await
		})
		.await
		.unwrap_err();
	let listed = store
		.read(async |tx| tx.imported_conversations().await)
		.await
		.unwrap();

	assert_eq!(
		(
			matches!(refused_import, StoreError::Integrity(_)),
			matches!(refused_continuation, StoreError::Integrity(_)),
			listed,
		),
		(
			true,
			true,
			vec![
				recorded(&first, Some(continuation.conversation_id)),
				recorded(&elsewhere, None),
			]
		)
	);
}
