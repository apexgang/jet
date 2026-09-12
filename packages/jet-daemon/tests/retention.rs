//! Black-box conformance tests for Jet Trash at the public Jet protocol
//! boundary: a real `jetd`, a real store, and the Rust client (ADR-0011,
//! ADR-0015).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	RetentionPolicy, RetentionProtection, SettingKey, SettingScope,
	SettingValue, TrashReason,
};
use pretty_assertions::assert_eq;
use std::time::Duration;
use support::start_jetd;
use uuid::Uuid;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Forgetting stages a Conversation under the Plane's grace period, the
/// Trash and the preview both show it, restoring empties the Trash, and a
/// Conversation with a Run is refused with the protection the preview
/// names.
#[tokio::test]
async fn forgetting_is_staged_previewed_and_restored_over_the_wire() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir().unwrap();
		let home = dir.path().join(".jet");
		let daemon = start_jetd(&home).await;
		let client = support::connect(&daemon, Uuid::new_v4()).await;
		client
			.set_setting(
				Uuid::now_v7(),
				SettingKey::RetentionTrashGraceDays,
				SettingScope::Plane,
				SettingValue::Count(7),
			)
			.await
			.unwrap();
		let idle = client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		let busy = client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		client
			.create_run(Uuid::now_v7(), busy.conversation_id)
			.await
			.unwrap();

		let entry = client
			.forget_conversation(Uuid::now_v7(), idle.conversation_id)
			.await
			.unwrap();
		let trash = client.conversation_trash().await.unwrap();
		let preview = client
			.retention_preview(idle.conversation_id)
			.await
			.unwrap();
		let refused = match client
			.forget_conversation(Uuid::now_v7(), busy.conversation_id)
			.await
		{
			Err(jet_client::ClientError::Remote(error)) => error.code,
			other => panic!("expected a stable refusal, got {other:?}"),
		};
		let protected = client
			.retention_preview(busy.conversation_id)
			.await
			.unwrap();
		client
			.restore_conversation(Uuid::now_v7(), idle.conversation_id)
			.await
			.unwrap();
		let emptied = client.conversation_trash().await.unwrap();

		assert_eq!(
			(
				entry.conversation_id,
				entry.reason,
				entry.expires_at_unix_ms - entry.trashed_at_unix_ms,
				trash.entries,
				preview,
				refused,
				protected.protections,
				emptied.entries,
			),
			(
				idle.conversation_id,
				TrashReason::ManualForget,
				7 * DAY_MS,
				vec![entry.clone()],
				jet_protocol::RetentionPreview {
					conversation_id: idle.conversation_id,
					protections: vec![],
					trash: Some(entry.clone()),
					audit_records: 1,
				},
				"retention.live_work".to_string(),
				vec![RetentionProtection::ActiveRun],
				vec![],
			)
		);
	})
	.await
	.unwrap();
}
