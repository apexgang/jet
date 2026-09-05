use std::path::Path;

use pretty_assertions::assert_eq;

use crate::test_support::{actor, request, start_core};
use crate::{
	AccountBinding, AccountBindingId, Command, CommandOutcome, Core, CoreError,
	CredentialItem, CredentialReference, CredentialSource, ErrorCategory,
	EventKind, EventSequence, ProviderAccount, ProviderId, Query, QueryResult,
};

/// The credential-store service every Jet Credential item lives under, as a
/// client reads it back and writes the secret Jet never sees.
const SERVICE: &str = "me.heeka.jet.credential";

fn anthropic() -> ProviderId {
	ProviderId("anthropic".into())
}

async fn start(dir: &tempfile::TempDir) -> Core {
	start_core(&dir.path().join("plane.sqlite3")).await
}

async fn bind(
	core: &Core,
	label: &str,
	provider_account: Option<&str>,
	credential: CredentialSource,
) -> Result<AccountBinding, CoreError> {
	bind_to(core, anthropic(), label, provider_account, credential).await
}

async fn bind_to(
	core: &Core,
	provider: ProviderId,
	label: &str,
	provider_account: Option<&str>,
	credential: CredentialSource,
) -> Result<AccountBinding, CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request(Command::BindAccount {
				provider,
				label: label.into(),
				provider_account: provider_account
					.map(|identity| ProviderAccount(identity.into())),
				credential,
			}),
		)
		.await?;
	let CommandOutcome::AccountBound(binding) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(binding)
}

async fn unbind(
	core: &Core,
	binding_id: AccountBindingId,
) -> Result<CommandOutcome, CoreError> {
	core.execute(&actor(), request(Command::UnbindAccount { binding_id }))
		.await
}

async fn bindings(core: &Core) -> Vec<AccountBinding> {
	let result = core.query(&actor(), Query::AccountBindings).await.unwrap();
	let QueryResult::AccountBindings(list) = result else {
		panic!("unexpected result {result:?}");
	};
	list.bindings
}

async fn events(core: &Core) -> Vec<EventKind> {
	let result = core
		.query(
			&actor(),
			Query::Events {
				after: EventSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::Events(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.events.into_iter().map(|event| event.kind).collect()
}

fn platform_item(binding_id: AccountBindingId) -> CredentialReference {
	CredentialReference::PlatformStore {
		item: CredentialItem {
			service: SERVICE.into(),
			account: binding_id.0.to_string(),
		},
	}
}

/// The Credential itself never enters Jet-owned state: a binding keeps the
/// non-secret metadata a person reads and an item name the core derives, so
/// the client that owns the secret knows where the platform store expects
/// it (ADR-0076).
#[tokio::test]
async fn a_binding_keeps_only_the_reference_its_credential_resolves_through() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	let bound = bind(
		&core,
		"Work",
		Some("acct-7"),
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap();
	let listed = bindings(&core).await;

	assert_eq!(
		(&bound, listed),
		(
			&AccountBinding {
				binding_id: bound.binding_id,
				provider: anthropic(),
				label: "Work".into(),
				provider_account: Some(ProviderAccount("acct-7".into())),
				credential: platform_item(bound.binding_id),
				created_at: bound.created_at,
			},
			vec![bound.clone()]
		)
	);
}

/// A Plane that cannot store a durable Credential still binds an account,
/// and the reference says plainly what the binding costs: an external
/// helper answers for it, the Harness authenticates itself, or the
/// Credential lives in one daemon's memory and no longer resolves after a
/// restart (ADR-0076).
#[tokio::test]
async fn every_credential_source_reports_the_limitation_it_carries() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let first = start_core(&path).await;

	let helper = bind(
		&first,
		"Vault",
		None,
		CredentialSource::ExternalHelper {
			helper: "op-read".into(),
		},
	)
	.await
	.unwrap();
	let native =
		bind(&first, "Codex login", None, CredentialSource::HarnessNative)
			.await
			.unwrap();
	let session = bind(&first, "Today", None, CredentialSource::SessionOnly)
		.await
		.unwrap();
	first.close().await;
	let second = start_core(&path).await;
	let after_restart =
		bind(&second, "Tomorrow", None, CredentialSource::SessionOnly)
			.await
			.unwrap();

	assert_eq!(
		[
			helper.credential,
			native.credential,
			session.credential,
			after_restart.credential
		],
		[
			CredentialReference::ExternalHelper {
				helper: "op-read".into()
			},
			CredentialReference::HarnessNative,
			CredentialReference::SessionOnly {
				established_at_daemon_start: 1
			},
			CredentialReference::SessionOnly {
				established_at_daemon_start: 2
			},
		]
	);
}

/// Bindings group into one Provider account only through an identity the
/// Provider supplies, so that identity is bound once per Provider. Bindings
/// without one are the user's to link and may repeat (ADR-0016).
#[tokio::test]
async fn a_provider_account_is_bound_once_per_provider() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let first = bind(
		&core,
		"Work",
		Some("acct-7"),
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap();

	let again = bind(
		&core,
		"Work again",
		Some("acct-7"),
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap_err();
	let elsewhere = bind_to(
		&core,
		ProviderId("openai".into()),
		"Work",
		Some("acct-7"),
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap();
	let unidentified =
		bind(&core, "Personal", None, CredentialSource::SessionOnly)
			.await
			.unwrap();
	let also_unidentified =
		bind(&core, "Personal", None, CredentialSource::SessionOnly)
			.await
			.unwrap();

	assert_eq!(
		(again.category, again.code.as_str(), bindings(&core).await),
		(
			ErrorCategory::Conflict,
			"account.already_bound",
			vec![first, elsewhere, unidentified, also_unidentified]
		)
	);
}

/// Binding metadata is text a person types and reads. Anything that is not
/// — an empty name, one longer than the bound, or one carrying the control
/// characters a pasted secret brings with it — is refused rather than
/// stored (ADR-0061, ADR-0076).
#[tokio::test]
async fn metadata_that_is_not_metadata_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	let provider = bind_to(
		&core,
		ProviderId("Anthropic Inc".into()),
		"Work",
		None,
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap_err();
	let label = bind(
		&core,
		"sk-live-4f9\nc2\u{7}",
		None,
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap_err();
	let identity = bind(
		&core,
		"Work",
		Some(&"a".repeat(129)),
		CredentialSource::PlatformStore,
	)
	.await
	.unwrap_err();
	let helper = bind(
		&core,
		"Vault",
		None,
		CredentialSource::ExternalHelper {
			helper: String::new(),
		},
	)
	.await
	.unwrap_err();

	assert_eq!(
		(
			[
				provider.category,
				label.category,
				identity.category,
				helper.category
			],
			[
				provider.code.as_str(),
				label.code.as_str(),
				identity.code.as_str(),
				helper.code.as_str()
			],
			bindings(&core).await
		),
		(
			[ErrorCategory::InvalidInput; 4],
			[
				"account.provider_unsupported",
				"account.label_unsupported",
				"account.provider_account_unsupported",
				"account.helper_unsupported"
			],
			vec![]
		)
	);
}

/// Jet forgets the reference and leaves the secret to the backend that
/// holds it, so unbinding hands the reference back to the client that owns
/// it. Jet never reaches into a credential backend itself (ADR-0076).
#[tokio::test]
async fn unbinding_hands_the_reference_back_and_forgets_the_binding() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let bound = bind(&core, "Work", None, CredentialSource::PlatformStore)
		.await
		.unwrap();

	let removed = unbind(&core, bound.binding_id).await.unwrap();
	let remaining = bindings(&core).await;
	let again = unbind(&core, bound.binding_id).await.unwrap_err();

	assert_eq!(
		(removed, remaining, again.category, again.code.as_str()),
		(
			CommandOutcome::AccountUnbound {
				binding_id: bound.binding_id,
				credential: platform_item(bound.binding_id),
			},
			vec![],
			ErrorCategory::NotFound,
			"account.not_found"
		)
	);
}

/// The journal says who bound what through which backend and nothing more.
/// The item name is derived from the binding, so even the reference is
/// absent from the record (ADR-0061, ADR-0076).
#[tokio::test]
async fn the_journal_records_the_binding_and_not_its_credential() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;
	let bound = bind(
		&core,
		"Work",
		Some("acct-7"),
		CredentialSource::ExternalHelper {
			helper: "op-read".into(),
		},
	)
	.await
	.unwrap();
	unbind(&core, bound.binding_id).await.unwrap();

	let recorded = events(&core).await;
	let payloads = recorded
		.iter()
		.map(|kind| kind.encode().unwrap())
		.map(|payload| (payload.kind, payload.payload))
		.collect::<Vec<_>>();

	assert_eq!(
		(recorded, payloads),
		(
			vec![
				EventKind::AccountBound {
					binding_id: bound.binding_id,
					provider: anthropic(),
					credential: CredentialSource::ExternalHelper {
						helper: "op-read".into()
					},
				},
				EventKind::AccountUnbound {
					binding_id: bound.binding_id,
				},
			],
			vec![
				(
					"account.bound".into(),
					serde_json::json!({
						"binding_id": bound.binding_id.0,
						"provider": "anthropic",
						"credential": {
							"source": "external_helper",
							"helper": "op-read"
						},
					})
				),
				(
					"account.unbound".into(),
					serde_json::json!({ "binding_id": bound.binding_id.0 })
				),
			]
		)
	);
}

/// The store outlives the daemon that wrote it, and a binding is Plane
/// state like any other (ADR-0016).
#[tokio::test]
async fn bindings_outlive_the_daemon_that_established_them() {
	let dir = tempfile::tempdir().unwrap();
	let path: &Path = &dir.path().join("plane.sqlite3");
	let first = start_core(path).await;
	let bound = bind(&first, "Work", None, CredentialSource::PlatformStore)
		.await
		.unwrap();
	first.close().await;

	let second = start_core(path).await;

	assert_eq!(bindings(&second).await, vec![bound]);
}
