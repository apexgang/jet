use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{AccountBindingRecord, CredentialSourceRecord, NewAccountBinding};
use crate::{Store, StoreError};

const NOW_UNIX_MS: i64 = 1_700_000_000_000;

fn binding(
	binding_id: Uuid,
	provider_account: Option<&str>,
	credential: CredentialSourceRecord,
) -> NewAccountBinding {
	NewAccountBinding {
		binding_id,
		provider: "anthropic".into(),
		label: "Work".into(),
		provider_account: provider_account.map(Into::into),
		credential,
		established_at_daemon_start: 1,
		created_at_unix_ms: NOW_UNIX_MS,
	}
}

fn recorded(binding: NewAccountBinding) -> AccountBindingRecord {
	AccountBindingRecord {
		binding_id: binding.binding_id,
		provider: binding.provider,
		label: binding.label,
		provider_account: binding.provider_account,
		credential: binding.credential,
		established_at_daemon_start: binding.established_at_daemon_start,
		created_at_unix_ms: binding.created_at_unix_ms,
	}
}

#[tokio::test]
async fn bindings_outlive_the_daemon_that_recorded_them() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let helper = binding(
		Uuid::now_v7(),
		Some("acct-7"),
		CredentialSourceRecord::ExternalHelper {
			helper: "op-read".into(),
		},
	);
	let session =
		binding(Uuid::now_v7(), None, CredentialSourceRecord::SessionOnly);

	let first = Store::open(&path).await.unwrap();
	first
		.write(async |tx| {
			tx.insert_account_binding(helper.clone()).await?;
			tx.insert_account_binding(session.clone()).await
		})
		.await
		.unwrap();
	first.close().await;

	let second = Store::open(&path).await.unwrap();
	let reopened = second
		.read(async |tx| tx.account_bindings().await)
		.await
		.unwrap();
	second
		.write(async |tx| tx.delete_account_binding(session.binding_id).await)
		.await
		.unwrap();
	let remaining = second
		.read(async |tx| tx.account_bindings().await)
		.await
		.unwrap();

	assert_eq!(
		(reopened, remaining),
		(
			vec![recorded(helper.clone()), recorded(session)],
			vec![recorded(helper)]
		)
	);
}

/// Bindings group into one Provider account only through an identity the
/// Provider supplies, so the store keeps that identity unique per Provider
/// even though the core refuses the duplicate first (ADR-0016).
#[tokio::test]
async fn one_provider_account_is_bound_at_most_once() {
	let dir = tempfile::tempdir().unwrap();
	let store = Store::open(&dir.path().join("plane.sqlite3"))
		.await
		.unwrap();
	let first = binding(
		Uuid::now_v7(),
		Some("acct-7"),
		CredentialSourceRecord::PlatformStore,
	);
	let again = binding(
		Uuid::now_v7(),
		Some("acct-7"),
		CredentialSourceRecord::PlatformStore,
	);
	store
		.write(async |tx| tx.insert_account_binding(first.clone()).await)
		.await
		.unwrap();

	let refused = store
		.write(async |tx| tx.insert_account_binding(again).await)
		.await
		.unwrap_err();
	let found = store
		.read(async |tx| tx.account_binding_for("anthropic", "acct-7").await)
		.await
		.unwrap();

	assert!(matches!(refused, StoreError::Integrity(_)), "{refused:?}");
	assert_eq!(found, Some(recorded(first)));
}
