//! Black-box Account binding conformance tests at the public Jet protocol
//! boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_client::ClientError;
use jet_protocol::{
	AccountBinding, ClientMessage, CredentialItem, CredentialReference,
	CredentialSource, ErrorCategory, QueryRequest,
	SETTINGS_AND_CAPABILITIES_MINOR, ServerHello, ServerMessage,
};
use pretty_assertions::assert_eq;
use support::{connect, handshake_raw, hello, start_jetd};
use uuid::Uuid;

/// The service every Jet Credential item lives under, as a client reads it
/// back to write the secret the Plane never sees.
const SERVICE: &str = "me.heeka.jet.credential";

fn platform_item(binding: &AccountBinding) -> CredentialReference {
	CredentialReference::PlatformStore {
		item: CredentialItem {
			service: SERVICE.into(),
			account: binding.binding_id.to_string(),
		},
	}
}

/// A binding is Plane state, and the reference it keeps is all the Plane
/// ever has: the secret belongs to the platform credential store, and the
/// client that owns it is told exactly where the Plane will look
/// (ADR-0016, ADR-0076).
#[tokio::test]
async fn a_binding_and_its_reference_outlive_the_daemon_that_stored_them() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();

	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	let bound = client
		.bind_account(
			Uuid::now_v7(),
			"anthropic",
			"Work",
			Some("acct-7"),
			CredentialSource::PlatformStore,
		)
		.await
		.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let resumed = client.account_bindings().await.unwrap();
	let forgotten = client
		.unbind_account(Uuid::now_v7(), bound.binding_id)
		.await
		.unwrap();
	let remaining = client.account_bindings().await.unwrap();

	assert_eq!(
		(&bound, resumed.bindings, forgotten, remaining.bindings),
		(
			&AccountBinding {
				binding_id: bound.binding_id,
				provider: "anthropic".into(),
				label: "Work".into(),
				provider_account: Some("acct-7".into()),
				credential: platform_item(&bound),
				created_at_unix_ms: bound.created_at_unix_ms,
			},
			vec![bound.clone()],
			platform_item(&bound),
			vec![]
		)
	);
}

/// Binding metadata is text a person types. A secret pasted into it carries
/// the control characters that no label does, and the Plane refuses it
/// instead of storing it (ADR-0076).
#[tokio::test]
async fn a_secret_pasted_into_binding_metadata_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client = connect(&daemon, Uuid::new_v4()).await;

	let error = client
		.bind_account(
			Uuid::now_v7(),
			"anthropic",
			"sk-live-4f9\nc2",
			None,
			CredentialSource::PlatformStore,
		)
		.await
		.unwrap_err();
	let stored = client.account_bindings().await.unwrap();

	let ClientError::Remote(error) = error else {
		panic!("expected a stable remote error, got {error:?}");
	};
	assert_eq!(
		(
			error.category,
			error.code.as_str(),
			error.retryable,
			stored.bindings
		),
		(
			ErrorCategory::InvalidInput,
			"account.label_unsupported",
			false,
			vec![]
		)
	);
}

/// A client that negotiated a minor without Account bindings is answered
/// with a stable refusal rather than a guess (ADR-0019, ADR-0094).
#[tokio::test]
async fn a_client_below_the_account_binding_minor_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut older = hello(Uuid::new_v4());
	older.minor = SETTINGS_AND_CAPABILITIES_MINOR;

	let (mut connection, welcome) = handshake_raw(&daemon, &older).await;
	assert!(
		matches!(welcome, ServerHello::Welcome { minor, .. }
			if minor == SETTINGS_AND_CAPABILITIES_MINOR),
		"expected a welcome at the older minor, got {welcome:?}"
	);
	connection
		.send(&ClientMessage::Query {
			id: 1,
			query: QueryRequest::AccountBindings,
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
			"the Account binding Query needs protocol minor 4"
		)
	);
}
