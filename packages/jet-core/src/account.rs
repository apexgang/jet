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

use std::time::SystemTime;

use jet_store::{
	AccountBindingRecord, CredentialSourceRecord, NewAccountBinding,
	WriteTransaction,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Actor;
use crate::capability::{CredentialStoreKind, CredentialStoreStatus};
use crate::command::CommandOutcome;
use crate::error::CoreError;
use crate::event::{EventKind, EventSequence, EventSubject};
use crate::system_time;

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
	tx.delete_account_binding(binding_id.0).await?;
	let event = EventKind::AccountUnbound { binding_id };
	tx.append_event(event.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
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
