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

use jet_store::{AccountBindingRecord, CredentialSourceRecord};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CoreError;
use crate::event::EventSequence;
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
	pub credential: CredentialReference,
	/// When the binding was recorded.
	pub created_at: SystemTime,
}

/// Every Account binding on the Plane, fenced by the journal position the
/// snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBindingList {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The bindings in the order they were established.
	pub bindings: Vec<AccountBinding>,
}

/// A validated Account binding, ready to be recorded.
pub(crate) struct PreparedBinding {
	pub(crate) provider: ProviderId,
	pub(crate) label: String,
	pub(crate) provider_account: Option<ProviderAccount>,
	pub(crate) credential: CredentialSource,
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
			credential: CredentialReference::from_record(
				binding_id,
				record.credential,
				record.established_at_daemon_start,
			),
			created_at: system_time(record.created_at_unix_ms),
		}
	}
}

/// Validates the non-secret metadata of a new binding and settles which
/// backend resolves it.
///
/// Everything a client supplies here is metadata a person reads. Nothing
/// carries the Credential itself, and the platform-store item is named by
/// the core rather than accepted, so a client cannot write secret material
/// into Jet-owned state through this Command (ADR-0076).
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when a value is empty, longer
/// than the bound on binding metadata, or not the kind of text that
/// metadata is.
pub(crate) fn prepare_binding(
	provider: ProviderId,
	label: String,
	provider_account: Option<ProviderAccount>,
	credential: CredentialSource,
) -> Result<PreparedBinding, CoreError> {
	require_provider(&provider.0)?;
	require_metadata("account.label_unsupported", "a binding label", &label)?;
	if let Some(ProviderAccount(account)) = &provider_account {
		require_metadata(
			"account.provider_account_unsupported",
			"a Provider account identity",
			account,
		)?;
	}
	if let CredentialSource::ExternalHelper { helper } = &credential {
		require_metadata(
			"account.helper_unsupported",
			"a credential helper name",
			helper,
		)?;
	}
	Ok(PreparedBinding {
		provider,
		label,
		provider_account,
		credential,
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
