//! Wire form of Plane-local Account bindings and the opaque Credential
//! references they resolve through (ADR-0016, ADR-0076).
//!
//! A binding carries the non-secret metadata a person reads and the
//! reference its Credential resolves through. The secret itself never
//! crosses this protocol: where the platform credential store holds it, the
//! Plane reports the item name it derived so the client that owns the
//! secret writes it where the Plane will look.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which backend a client asks a new binding to resolve its Credential
/// through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum CredentialSource {
	/// The platform credential store, under an item the Plane names.
	PlatformStore,
	/// An explicitly configured external authentication helper, invoked at
	/// the moment of use.
	ExternalHelper {
		/// The helper's non-secret name.
		helper: String,
	},
	/// Native Harness authentication supplied by the environment the
	/// Harness is launched in. Jet references nothing of its own.
	HarnessNative,
	/// Memory of one daemon process, which a restart invalidates.
	SessionOnly,
}

/// The opaque reference a Plane keeps for one binding's Credential, and the
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
	/// Native Harness authentication; the Plane references nothing.
	HarnessNative,
	/// Memory of the daemon start that established it.
	SessionOnly {
		/// The daemon start whose memory holds the Credential. A later
		/// start holds nothing, so the binding must be established again.
		established_at_daemon_start: u64,
	},
}

/// One platform credential-store item. The Plane names it after the binding
/// alone, so no client-supplied text enters Plane-owned state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialItem {
	/// The service every Jet Credential item lives under.
	pub service: String,
	/// The item within that service.
	pub account: String,
}

/// One Plane-local Account binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBinding {
	/// Durable identity.
	pub binding_id: Uuid,
	/// The Provider this binding authenticates to, such as `anthropic`.
	pub provider: String,
	/// The user-facing name of the binding.
	pub label: String,
	/// The Provider's own account identity, when it supplies one. Bindings
	/// group into one Provider account automatically only when they share
	/// one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider_account: Option<String>,
	/// The reference its Credential resolves through.
	pub credential: CredentialReference,
	/// When the binding was established, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
}

/// Every Account binding on one Plane, fenced by a journal cursor
/// (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBindingList {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The bindings in the order they were established.
	pub bindings: Vec<AccountBinding>,
}
