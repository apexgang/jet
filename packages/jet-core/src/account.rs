//! Plane-local Account bindings and the opaque Credential references they
//! resolve through (ADR-0016, ADR-0076).
//!
//! A binding says that one Provider account may be used on this Plane. It
//! is Plane-local and authoritative here: nothing synchronizes it, and the
//! Provider account a GUI client shows is the grouping it makes from the
//! bindings of every Plane it is connected to.
//!
//! Jet stores only a Credential reference. Where the platform credential
//! store resolves it, the core names the item and reports that name, so the
//! client that owns the secret knows where to put it and Jet never holds
//! it. Tokens, keys, passwords, and authentication callbacks never reach
//! Jet-owned state, and there is no plaintext fallback when a backend
//! cannot be reached.

use crate::{
	Actor,
	audit::{self, AuditDecision, AuditSubject, Decision},
	capability::{CredentialStoreKind, CredentialStoreStatus},
	command::CommandOutcome,
	error::CoreError,
	event::{EventKind, EventSequence, EventSubject},
	system_time,
};
use jet_store::{
	AccountBindingRecord, CredentialSourceRecord, NewAccountBinding,
	WriteTransaction,
};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

/// The credential-store service every Jet Credential item lives under. It
/// is Jet's own application identity, so a Credential item is recognizable
/// in the platform's own credential user interface.
const CREDENTIAL_SERVICE: &str = "me.heeka.jet.credential";

/// Longest Provider name a binding may carry.
const MAX_PROVIDER_CHARS: usize = 64;

/// Longest label, Provider account identity, or helper name a binding may
/// carry. Each is metadata a person reads, not a payload.
const MAX_METADATA_CHARS: usize = 128;

/// Durable identity of one Account binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountBindingId(pub Uuid);

/// A vendor that supplies inference models, such as `anthropic`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

/// The stable account identity a Provider supplies. Bindings group into one
/// Provider account automatically only when they share one; without it the
/// user links them explicitly (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderAccount(pub String);

/// Which backend a client asks a new binding to resolve its Credential
/// through (ADR-0076).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum CredentialSource {
	/// The platform credential store, under an item the core names.
	PlatformStore,
	/// An explicitly configured external authentication helper, invoked at
	/// the moment of use. Jet keeps its name and never sees its answer.
	ExternalHelper {
		/// The helper's non-secret name.
		helper: String,
	},
	/// Native Harness authentication supplied by the environment the
	/// Harness is launched in, such as an SSH agent or a Harness that
	/// already holds its own login. Jet holds no reference of its own.
	HarnessNative,
	/// Memory of one daemon process, which a restart invalidates. The
	/// limitation is reported rather than worked around.
	SessionOnly,
}

/// The opaque reference Jet stores for one binding's Credential, and the
/// place the secret it resolves to belongs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum CredentialReference {
	/// An item of the platform credential store.
	PlatformStore {
		/// The item the backend resolves.
		item: CredentialItem,
	},
	/// An external authentication helper.
	ExternalHelper {
		/// The helper's non-secret name.
		helper: String,
	},
	/// Native Harness authentication; Jet references nothing.
	HarnessNative,
	/// Memory of the daemon start that established it.
	SessionOnly {
		/// The daemon start whose memory holds the Credential. A later
		/// start holds nothing, so the binding must be established again.
		established_at_daemon_start: u64,
	},
}

/// One platform credential-store item. The core names it after the binding
/// alone, so no client-supplied text enters Jet-owned state and no two
/// bindings can address the same secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialItem {
	/// The service every Jet Credential item lives under.
	pub service: String,
	/// The item within that service: the binding's own identity.
	pub account: String,
}

/// One Plane-local Account binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBinding {
	/// Durable identity.
	pub binding_id: AccountBindingId,
	/// The Provider this binding authenticates to.
	pub provider: ProviderId,
	/// The user-facing name of the binding.
	pub label: String,
	/// The Provider's own account identity, when it supplies one.
	pub provider_account: Option<ProviderAccount>,
	/// The opaque reference its Credential resolves through.
	pub credential_reference: CredentialReference,
	/// When the binding was recorded.
	pub created_at: SystemTime,
}

/// Whether one binding's Credential can be resolved right now, and what
/// has to happen when it cannot.
///
/// `jetd` never asks a person for a secret: it has no interface to ask
/// through and no place to keep the answer. A backend that will not answer
/// therefore becomes one of these, and the GUI that has a user in front of
/// it starts the operating system's own unlock flow (ADR-0076).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialState {
	/// The Plane can see the backend, so the Credential resolves at the
	/// moment of use.
	Resolvable,
	/// The Plane holds no evidence either way. An external helper and
	/// native Harness authentication answer only when they are invoked, and
	/// invoking one early would run somebody's helper for no reason, so
	/// work that needs the Credential finds out when it asks.
	ResolvedAtUse,
	/// The backend is present but locked. Work that needs the Credential
	/// waits until the user unlocks it.
	WaitingForUnlock {
		/// The store that is locked.
		kind: CredentialStoreKind,
	},
	/// The backend cannot be reached on this Plane at all, so this binding
	/// cannot be used until secure storage is set up.
	Unavailable {
		/// The store that was expected.
		kind: CredentialStoreKind,
	},
	/// A session-only Credential that an earlier daemon start established.
	/// This one holds nothing, so it must be established again.
	InvalidatedByRestart,
}

/// One Account binding beside the state of the Credential it resolves. The
/// binding is durable Plane state; the state beside it is observed when the
/// Query runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBindingStatus {
	/// The binding as the Plane recorded it.
	pub binding: AccountBinding,
	/// Whether its Credential can be resolved right now.
	pub credential_state: CredentialState,
}

/// Every Account binding on the Plane, fenced by the journal position the
/// snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBindingList {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The bindings in the order they were established.
	pub bindings: Vec<AccountBindingStatus>,
}

/// What a client asked one new binding to be, before the core has checked
/// that it is the metadata a binding carries.
pub(crate) struct Requested {
	/// The Provider the binding authenticates to.
	pub(crate) provider: ProviderId,
	/// The user-facing name of the binding.
	pub(crate) label: String,
	/// The Provider's own account identity, when it supplies one.
	pub(crate) provider_account: Option<ProviderAccount>,
	/// The backend that is to resolve the binding's Credential.
	pub(crate) credential_source: CredentialSource,
}

impl CredentialSource {
	/// The durable form the store keeps this source in.
	pub(crate) fn record(&self) -> CredentialSourceRecord {
		match self {
			Self::PlatformStore => CredentialSourceRecord::PlatformStore,
			Self::ExternalHelper { helper } => {
				CredentialSourceRecord::ExternalHelper {
					helper: helper.clone(),
				}
			}
			Self::HarnessNative => CredentialSourceRecord::HarnessNative,
			Self::SessionOnly => CredentialSourceRecord::SessionOnly,
		}
	}
}

impl CredentialItem {
	/// The item that resolves `binding_id`.
	fn for_binding(binding_id: AccountBindingId) -> Self {
		Self {
			service: CREDENTIAL_SERVICE.into(),
			account: binding_id.0.to_string(),
		}
	}
}

impl CredentialReference {
	/// The reference a stored source resolves through, named for the
	/// binding that owns it.
	fn from_record(
		binding_id: AccountBindingId,
		source: CredentialSourceRecord,
		established_at_daemon_start: u64,
	) -> Self {
		match source {
			CredentialSourceRecord::PlatformStore => Self::PlatformStore {
				item: CredentialItem::for_binding(binding_id),
			},
			CredentialSourceRecord::ExternalHelper { helper } => {
				Self::ExternalHelper { helper }
			}
			CredentialSourceRecord::HarnessNative => Self::HarnessNative,
			CredentialSourceRecord::SessionOnly => Self::SessionOnly {
				established_at_daemon_start,
			},
		}
	}
}

impl From<AccountBindingRecord> for AccountBinding {
	fn from(record: AccountBindingRecord) -> Self {
		let binding_id = AccountBindingId(record.binding_id);
		Self {
			binding_id,
			provider: ProviderId(record.provider),
			label: record.label,
			provider_account: record.provider_account.map(ProviderAccount),
			credential_reference: CredentialReference::from_record(
				binding_id,
				record.credential,
				record.established_at_daemon_start,
			),
			created_at: system_time(record.created_at_unix_ms),
		}
	}
}

impl CredentialState {
	/// The state of `reference` on a Plane whose credential store was last
	/// seen as `store` and whose current daemon start is `daemon_start`.
	pub(crate) fn of(
		reference: &CredentialReference,
		store: CredentialStoreStatus,
		daemon_start: u64,
	) -> Self {
		match reference {
			CredentialReference::PlatformStore { .. } => match store {
				CredentialStoreStatus::Available { .. } => Self::Resolvable,
				CredentialStoreStatus::Locked { kind } => {
					Self::WaitingForUnlock { kind }
				}
				CredentialStoreStatus::Unavailable { kind } => {
					Self::Unavailable { kind }
				}
			},
			CredentialReference::ExternalHelper { .. }
			| CredentialReference::HarnessNative => Self::ResolvedAtUse,
			CredentialReference::SessionOnly {
				established_at_daemon_start,
			} => {
				if *established_at_daemon_start == daemon_start {
					Self::Resolvable
				} else {
					Self::InvalidatedByRestart
				}
			}
		}
	}
}

/// Checks that a requested binding carries the metadata a binding carries.
///
/// The Credential has no field to arrive through: no parameter accepts
/// secret material, and the platform-store item is named by the core rather
/// than taken from the request, so nothing a client sends becomes a secret
/// Jet holds (ADR-0076). What is checked here is narrower — that a label,
/// a Provider account identity, and a helper name are bounded text without
/// control characters. A person who types a token into a label still gets
/// it stored as the label they typed; the guarantee is that Jet has no
/// place for a secret, not that it can recognize one.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when a value is empty, longer
/// than the bound on binding metadata, or not the kind of text that
/// metadata is.
fn require_binding_metadata(requested: &Requested) -> Result<(), CoreError> {
	require_provider(&requested.provider.0)?;
	require_metadata(
		"account.label_unsupported",
		"a binding label",
		&requested.label,
	)?;
	if let Some(ProviderAccount(account)) = &requested.provider_account {
		require_metadata(
			"account.provider_account_unsupported",
			"a Provider account identity",
			account,
		)?;
	}
	if let CredentialSource::ExternalHelper { helper } =
		&requested.credential_source
	{
		require_metadata(
			"account.helper_unsupported",
			"a credential helper name",
			helper,
		)?;
	}
	Ok(())
}

/// Records one Plane-local binding and journals it.
///
/// A Provider account identity is what a GUI groups bindings by, not a key,
/// so one Plane may hold several bindings for one Provider account — the
/// same account through the platform store and through a helper, for
/// instance (ADR-0016).
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the metadata is not the
/// metadata a binding carries, or a store category when the row cannot be
/// written.
pub(crate) async fn bind(
	tx: &mut WriteTransaction,
	actor: &Actor,
	requested: Requested,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	require_binding_metadata(&requested)?;
	let Requested {
		provider,
		label,
		provider_account,
		credential_source,
	} = requested;
	// The daemon start that establishes the binding is what tells a later
	// start that a session-only Credential is no longer the one it holds.
	let established_at_daemon_start = tx.plane().await?.daemon_starts;
	let binding: AccountBinding = tx
		.insert_account_binding(NewAccountBinding {
			binding_id: Uuid::now_v7(),
			provider: provider.0.clone(),
			label,
			provider_account: provider_account
				.map(|ProviderAccount(identity)| identity),
			credential: credential_source.record(),
			established_at_daemon_start,
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	// ASVS 8.3.4 and 14.1.4: the journal records who bound what through
	// which backend, and no part of the Credential itself.
	let event = EventKind::AccountBound {
		binding_id: binding.binding_id,
		provider,
		credential_source,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	// ASVS 16.2.1: widening what may authenticate on this Plane is a
	// Security audit decision, not only journal history (ADR-0105).
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::AccountBound,
			AuditSubject::AccountBinding(binding.binding_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::AccountBound(binding))
}

/// Forgets one binding and hands its reference back, so the client that
/// owns the secret can remove it from the backend that holds it. Jet never
/// reaches into a credential backend itself (ADR-0076).
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when the Plane has no such binding,
/// or a store category when the row cannot be removed.
pub(crate) async fn unbind(
	tx: &mut WriteTransaction,
	actor: &Actor,
	binding_id: AccountBindingId,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Some(record) = tx.account_binding(binding_id.0).await? else {
		return Err(CoreError::not_found(
			"account.not_found",
			"the Account binding does not exist",
		));
	};
	let binding: AccountBinding = record.into();
	tx.delete_account_binding(binding_id.0, now_unix_ms).await?;
	let event = EventKind::AccountUnbound { binding_id };
	tx.append_event(event.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::AccountUnbound,
			AuditSubject::AccountBinding(binding_id),
		),
		now_unix_ms,
	)
	.await?;
	Ok(CommandOutcome::AccountUnbound {
		binding_id,
		credential_reference: binding.credential_reference,
	})
}

/// Refuses a Provider name that is not the stable lowercase identity a
/// Craft and a GUI both spell the same way.
fn require_provider(provider: &str) -> Result<(), CoreError> {
	let supported = !provider.is_empty()
		&& provider.chars().count() <= MAX_PROVIDER_CHARS
		&& provider.chars().all(|character| {
			character.is_ascii_lowercase()
				|| character.is_ascii_digit()
				|| matches!(character, '-' | '.' | '_')
		});
	if supported {
		return Ok(());
	}
	Err(CoreError::invalid_input(
		"account.provider_unsupported",
		format!(
			"a Provider is at most {MAX_PROVIDER_CHARS} characters of \
			 lowercase letters, digits, and -._"
		),
	))
}

/// Refuses binding metadata that is empty, too long, or carries control
/// characters, which no name a person types does.
fn require_metadata(
	code: &'static str,
	described: &str,
	value: &str,
) -> Result<(), CoreError> {
	let supported = !value.trim().is_empty()
		&& value.chars().count() <= MAX_METADATA_CHARS
		&& !value.chars().any(char::is_control);
	if supported {
		return Ok(());
	}
	Err(CoreError::invalid_input(
		code,
		format!(
			"{described} is between one and {MAX_METADATA_CHARS} characters \
			 and holds no control characters"
		),
	))
}

#[cfg(test)]
pub(crate) mod tests {
	use std::path::Path;
	use std::sync::Arc;
	use std::time::{Duration, SystemTime, UNIX_EPOCH};

	use pretty_assertions::assert_eq;

	use crate::capability::CredentialStoreKind;
	use crate::test_support::{
		FixedProbe, ManualClock, actor, equipped, locked, request, start_core,
		start_core_with, stripped,
	};
	use crate::{
		AccountBinding, AccountBindingId, AccountBindingStatus,
		CapabilityObservation, Command, CommandOutcome, Core, CoreError,
		CredentialItem, CredentialReference, CredentialSource, CredentialState,
		DegradedCondition, ErrorCategory, EventKind, EventSequence,
		ProviderAccount, ProviderId, Query, QueryResult,
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
		credential_source: CredentialSource,
	) -> Result<AccountBinding, CoreError> {
		bind_to(
			core,
			anthropic(),
			label,
			provider_account,
			credential_source,
		)
		.await
	}

	async fn bind_to(
		core: &Core,
		provider: ProviderId,
		label: &str,
		provider_account: Option<&str>,
		credential_source: CredentialSource,
	) -> Result<AccountBinding, CoreError> {
		let outcome = core
			.execute(
				&actor(),
				request(Command::BindAccount {
					provider,
					label: label.into(),
					provider_account: provider_account
						.map(|identity| ProviderAccount(identity.into())),
					credential_source,
				}),
			)
			.await?;
		let CommandOutcome::AccountBound(binding) = outcome else {
			panic!("expected CommandOutcome::AccountBound");
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

	async fn bindings(core: &Core) -> Vec<AccountBindingStatus> {
		observed_bindings(core, CapabilityObservation::LastObserved).await
	}

	async fn observed_bindings(
		core: &Core,
		observation: CapabilityObservation,
	) -> Vec<AccountBindingStatus> {
		let result = core
			.query(&actor(), Query::AccountBindings { observation })
			.await
			.unwrap();
		let QueryResult::AccountBindings(list) = result else {
			panic!("expected QueryResult::AccountBindings");
		};
		list.bindings
	}

	async fn credential_states(core: &Core) -> Vec<CredentialState> {
		bindings(core)
			.await
			.into_iter()
			.map(|status| status.credential_state)
			.collect()
	}

	/// One binding whose Credential the Plane can resolve right now.
	fn resolvable(binding: AccountBinding) -> AccountBindingStatus {
		AccountBindingStatus {
			binding,
			credential_state: CredentialState::Resolvable,
		}
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
			panic!("expected QueryResult::Events");
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
	async fn a_binding_keeps_only_the_reference_its_credential_resolves_through()
	 {
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
					credential_reference: platform_item(bound.binding_id),
					created_at: bound.created_at,
				},
				vec![resolvable(bound.clone())]
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
		let session =
			bind(&first, "Today", None, CredentialSource::SessionOnly)
				.await
				.unwrap();
		first.close().await;
		let second = start_core(&path).await;
		let after_restart =
			bind(&second, "Tomorrow", None, CredentialSource::SessionOnly)
				.await
				.unwrap();
		let states = credential_states(&second).await;

		assert_eq!(
			(
				[
					helper.credential_reference,
					native.credential_reference,
					session.credential_reference,
					after_restart.credential_reference
				],
				states
			),
			(
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
				],
				vec![
					// A helper and a Harness answer only when they are asked,
					// and the Plane has not asked.
					CredentialState::ResolvedAtUse,
					CredentialState::ResolvedAtUse,
					// The daemon start whose memory held this one is gone.
					CredentialState::InvalidatedByRestart,
					CredentialState::Resolvable,
				]
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
					credential_reference: platform_item(bound.binding_id),
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
						credential_source: CredentialSource::ExternalHelper {
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
							"credential_source": {
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

		assert_eq!(bindings(&second).await, vec![resolvable(bound)]);
	}

	/// The one instant every observation in these tests is taken at.
	fn observed_at() -> SystemTime {
		UNIX_EPOCH + Duration::from_secs(1_700_000_000)
	}

	async fn start_observing(
		dir: &tempfile::TempDir,
		probe: Arc<FixedProbe>,
	) -> Core {
		start_core_with(
			&dir.path().join("plane.sqlite3"),
			ManualClock::at(observed_at()),
			probe,
		)
		.await
	}

	async fn degraded(core: &Core) -> Vec<DegradedCondition> {
		let result = core
			.query(
				&actor(),
				Query::Capabilities {
					observation: CapabilityObservation::LastObserved,
				},
			)
			.await
			.unwrap();
		let QueryResult::Capabilities(snapshot) = result else {
			panic!("expected QueryResult::Capabilities");
		};
		snapshot.degraded
	}

	/// A locked backend hides the secrets it holds, not the store itself: the
	/// binding is recorded and reports that it waits for the unlock the user
	/// performs through the operating system. `jetd` never asks for that itself
	/// (ADR-0076).
	#[tokio::test]
	async fn a_locked_credential_store_makes_a_binding_wait() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(locked());
		let core = start_observing(&dir, Arc::clone(&probe)).await;

		let bound = bind(&core, "Work", None, CredentialSource::PlatformStore)
			.await
			.unwrap();
		let while_locked = credential_states(&core).await;
		let conditions = degraded(&core).await;
		probe.answer_with(equipped());
		let after_unlocking =
			observed_bindings(&core, CapabilityObservation::Fresh).await;

		assert_eq!(
			(while_locked, conditions, after_unlocking),
			(
				vec![CredentialState::WaitingForUnlock {
					kind: CredentialStoreKind::SecretService
				}],
				vec![DegradedCondition::CredentialStoreLocked {
					kind: CredentialStoreKind::SecretService
				}],
				vec![resolvable(bound)]
			)
		);
	}

	/// A Plane with no credential store cannot hold a Credential durably, and
	/// Jet keeps no secret of its own instead: the durable binding is refused
	/// with what the Plane cannot do, while the honest session-only
	/// arrangement remains available (ADR-0076, ADR-0086).
	#[tokio::test]
	async fn a_plane_without_a_credential_store_refuses_a_durable_binding() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(equipped());
		let core = start_observing(&dir, Arc::clone(&probe)).await;
		let bound = bind(&core, "Work", None, CredentialSource::PlatformStore)
			.await
			.unwrap();
		probe.answer_with(stripped());

		let refused =
			bind(&core, "Also work", None, CredentialSource::PlatformStore)
				.await
				.unwrap_err();
		let session = bind(&core, "Today", None, CredentialSource::SessionOnly)
			.await
			.unwrap();
		let states = credential_states(&core).await;

		assert_eq!(
			(
				refused.category,
				refused.code.as_str(),
				refused.message,
				states
			),
			(
				ErrorCategory::Unavailable,
				"capability.unavailable",
				"this Plane cannot use the platform credential store right now"
					.into(),
				vec![
					CredentialState::Unavailable {
						kind: CredentialStoreKind::SecretService
					},
					CredentialState::Resolvable,
				]
			)
		);
		assert_eq!(
			bindings(&core)
				.await
				.into_iter()
				.map(|status| status.binding)
				.collect::<Vec<_>>(),
			vec![bound, session]
		);
	}

	/// A session-only Credential lives in the memory of the daemon start that
	/// established it. The binding survives a restart; the Credential does not,
	/// and the Plane says so rather than pretending otherwise (ADR-0076).
	#[tokio::test]
	async fn a_session_only_credential_does_not_outlive_its_daemon_start() {
		let dir = tempfile::tempdir().unwrap();
		let path: &Path = &dir.path().join("plane.sqlite3");
		let first = start_core(path).await;
		let bound = bind(&first, "Today", None, CredentialSource::SessionOnly)
			.await
			.unwrap();
		let while_held = credential_states(&first).await;
		first.close().await;

		let second = start_core(path).await;
		let after_restart = bindings(&second).await;

		assert_eq!(
			(while_held, after_restart),
			(
				vec![CredentialState::Resolvable],
				vec![AccountBindingStatus {
					binding: bound,
					credential_state: CredentialState::InvalidatedByRestart,
				}]
			)
		);
	}
}
