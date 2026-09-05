//! Black-box Setting conformance tests at the public Jet protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	ClientMessage, ErrorCategory, MULTIPLEXED_STREAMS_MINOR, QueryRequest,
	ResolvedSetting, RetentionPolicy, ServerHello, ServerMessage, SettingKey,
	SettingScope, SettingSelection, SettingSource, SettingValue,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, start_jetd};
use uuid::Uuid;

fn resolved(
	key: SettingKey,
	value: SettingValue,
	source: SettingSource,
) -> ResolvedSetting {
	ResolvedSetting { key, value, source }
}

#[tokio::test]
async fn a_stored_setting_outlives_the_daemon_that_stored_it() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let key = SettingKey::UtilityAutomaticNaming;

	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let scope = SettingScope::Conversation {
		conversation_id: conversation.conversation_id,
	};
	client
		.set_setting(
			Uuid::now_v7(),
			key,
			SettingScope::Plane,
			SettingValue::Flag(false),
		)
		.await
		.unwrap();
	client
		.set_setting(Uuid::now_v7(), key, scope, SettingValue::Flag(true))
		.await
		.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let resumed = client
		.settings(scope, SettingSelection::Key { key })
		.await
		.unwrap();
	client
		.clear_setting(Uuid::now_v7(), key, scope)
		.await
		.unwrap();
	let cleared = client
		.settings(scope, SettingSelection::Key { key })
		.await
		.unwrap();

	assert_eq!(
		(resumed.settings, cleared.settings, cleared.scope),
		(
			vec![resolved(
				key,
				SettingValue::Flag(true),
				SettingSource::Scope { scope }
			)],
			vec![resolved(
				key,
				SettingValue::Flag(false),
				SettingSource::Scope {
					scope: SettingScope::Plane
				}
			)],
			scope
		)
	);
}

#[tokio::test]
async fn a_setting_restricted_to_the_plane_is_refused_elsewhere() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;
	let client = connect(&daemon, Uuid::new_v4()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let scope = SettingScope::Conversation {
		conversation_id: conversation.conversation_id,
	};

	let error = client
		.set_setting(
			Uuid::now_v7(),
			SettingKey::GitMessageInstructions,
			scope,
			SettingValue::Text("Explain why, not what".into()),
		)
		.await
		.unwrap_err();

	let ClientError::Remote(error) = error else {
		panic!("expected a stable remote error, got {error:?}");
	};
	assert_eq!(
		(error.category, error.code.as_str(), error.retryable),
		(
			ErrorCategory::InvalidInput,
			"setting.scope_unsupported",
			false
		)
	);
}

/// A client that negotiated a minor without Settings is answered with a
/// stable refusal rather than a guess (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_settings_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let daemon = start_jetd(&home).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = MULTIPLEXED_STREAMS_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == MULTIPLEXED_STREAMS_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::Settings {
				scope: SettingScope::Plane,
				selection: SettingSelection::All,
			},
		})
		.await;

	let ServerMessage::Error { id, error } = connection.receive().await else {
		panic!("expected a refusal");
	};
	assert_eq!(
		(
			id,
			error.category,
			error.code.as_str(),
			error.message.as_str()
		),
		(
			Some(1),
			ErrorCategory::Incompatible,
			"protocol.unsupported_minor",
			"Setting Queries needs protocol minor 3"
		)
	);
}
