//! Black-box conformance tests for Autodelete rules at the public Jet
//! protocol boundary: a real `jetd`, a real store, and the Rust client
//! (ADR-0015, ADR-0099).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{AutodeleteRuleState, AutodeleteScope, RetentionPolicy};
use pretty_assertions::assert_eq;
use std::time::Duration;
use support::start_jetd;
use uuid::Uuid;

/// With Autodelete compilation disabled, the compiled rule fails closed
/// as refused and cannot be approved; set by hand it becomes a draft, an
/// approval naming a reading other than the one shown is refused, and the
/// shown one can be approved and authorized to delete everywhere. Deleting
/// the rule leaves nothing listed.
#[tokio::test]
async fn a_rule_fails_closed_then_is_edited_approved_and_removed() {
	tokio::time::timeout(Duration::from_secs(60), async {
		let dir = tempfile::tempdir().unwrap();
		let home = dir.path().join(".jet");
		let daemon = start_jetd(&home).await;
		let client = support::connect(&daemon, Uuid::new_v4()).await;
		let busy = client
			.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
			.await
			.unwrap();
		client
			.create_run(Uuid::now_v7(), busy.conversation_id)
			.await
			.unwrap();
		let rule_id = Uuid::now_v7();

		let compiled = client
			.compile_autodelete_rule(
				Uuid::now_v7(),
				rule_id,
				"Forget anything idle for a month".into(),
			)
			.await
			.unwrap();
		let refused = loop {
			let rules = client.autodelete_rules().await.unwrap();
			match &rules.rules[..] {
				[preview]
					if preview.rule.state != AutodeleteRuleState::Compiling =>
				{
					break preview.clone();
				}
				_ => tokio::time::sleep(Duration::from_millis(50)).await,
			}
		};
		let cannot_approve = match client
			.approve_autodelete_rule(Uuid::now_v7(), rule_id, 1)
			.await
		{
			Err(jet_client::ClientError::Remote(error)) => error.code,
			other => panic!("expected a stable refusal, got {other:?}"),
		};
		// A day is the shortest inactivity and nothing created this second
		// has been idle that long, so the draft selects nothing yet; the
		// state machine is what is under test here.
		let edited = client
			.set_autodelete_rule_inactive_days(Uuid::now_v7(), rule_id, 1)
			.await
			.unwrap();
		let unseen = match client
			.approve_autodelete_rule(Uuid::now_v7(), rule_id, 7)
			.await
		{
			Err(jet_client::ClientError::Remote(error)) => error.code,
			other => panic!("expected a stable refusal, got {other:?}"),
		};
		let approved = client
			.approve_autodelete_rule(Uuid::now_v7(), rule_id, 1)
			.await
			.unwrap();
		let everywhere = client
			.authorize_autodelete_everywhere(Uuid::now_v7(), rule_id)
			.await
			.unwrap();
		let listed = client.autodelete_rules().await.unwrap();
		client
			.delete_autodelete_rule(Uuid::now_v7(), rule_id)
			.await
			.unwrap();
		let emptied = client.autodelete_rules().await.unwrap();

		assert_eq!(
			(
				compiled.state,
				refused.rule.state,
				refused.candidates,
				cannot_approve,
				edited.state,
				edited.scope,
				unseen,
				approved.state,
				approved.scope,
				everywhere.scope,
				listed
					.rules
					.iter()
					.map(|preview| (
						preview.rule.rule_id,
						preview.rule.scope,
						preview.candidates.len()
					))
					.collect::<Vec<_>>(),
				emptied.rules,
			),
			(
				AutodeleteRuleState::Compiling,
				AutodeleteRuleState::Refused {
					reason: "utility.disabled".into()
				},
				vec![],
				"autodelete.refused".to_string(),
				AutodeleteRuleState::Draft { inactive_days: 1 },
				AutodeleteScope::Forget,
				"autodelete.interpretation_changed".to_string(),
				AutodeleteRuleState::Approved {
					inactive_days: 1,
					approved_at_unix_ms: approved.updated_at_unix_ms,
				},
				AutodeleteScope::Forget,
				AutodeleteScope::Everywhere,
				vec![(rule_id, AutodeleteScope::Everywhere, 0)],
				vec![],
			)
		);
	})
	.await
	.unwrap();
}
