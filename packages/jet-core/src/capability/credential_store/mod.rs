//! The platform credential store, observed by speaking to it (ADR-0076).
//!
//! The Plane looks *into* its credential backend rather than *for* it: on
//! Linux it speaks the Secret Service D-Bus interface, on macOS it calls
//! the Security framework. Two things never happen here. The probe never
//! prompts, because `jetd` has no person to ask and a desktop dialog opened
//! by a daemon is the failure ADR-0076 names; and it never starts a store
//! that is not running, because starting one can open its first-run
//! dialog. Both are reported instead.
//!
//! Observing is a read: it tells an open store from a locked one and from
//! none at all, and it happens with every Capability snapshot and every
//! Command revalidation. Verifying is a round trip: a probe item is
//! created, read back, and deleted, which is the proof durable Pairing
//! waits for. Verifying writes into a person's keyring, so it happens only
//! when a caller asks.

#[cfg(target_os = "macos")]
mod keychain;
#[cfg(not(target_os = "macos"))]
mod secret_service;

use crate::capability::{
	CredentialStoreKind, CredentialStoreStatus, CredentialStoreVerification,
};
use std::time::Duration;

/// Which store this platform resolves through.
#[cfg(target_os = "macos")]
pub(crate) const KIND: CredentialStoreKind = CredentialStoreKind::AppleKeychain;
/// Which store this platform resolves through.
#[cfg(not(target_os = "macos"))]
pub(crate) const KIND: CredentialStoreKind = CredentialStoreKind::SecretService;

/// How long the store gets to answer one probe. A store that hangs is one
/// the Plane cannot use right now, and a probe that waited on it would
/// stall every Command that revalidates the Capability.
const STORE_TIMEOUT: Duration = Duration::from_secs(5);

/// What the probe item is filed under. The service is Jet's own and the
/// account says it is a probe, so it is never mistaken for a binding's
/// Credential.
const PROBE_SERVICE: &str = "me.heeka.jet.credential-store";
const PROBE_ACCOUNT: &str = "probe";
/// The name a person sees if a probe item is ever left behind. The
/// Keychain names an item by its service and account instead.
#[cfg(not(target_os = "macos"))]
const PROBE_LABEL: &str = "Jet credential store probe";

/// Whether the store can be reached, and whether it will answer.
pub(crate) async fn observe() -> CredentialStoreStatus {
	#[cfg(target_os = "macos")]
	{
		keychain::observe().await
	}
	#[cfg(not(target_os = "macos"))]
	{
		secret_service::observe().await
	}
}

/// Creates, reads back, and deletes one probe item.
pub(crate) async fn verify() -> CredentialStoreVerification {
	#[cfg(target_os = "macos")]
	{
		keychain::verify().await
	}
	#[cfg(not(target_os = "macos"))]
	{
		secret_service::verify().await
	}
}

/// A throwaway secret for one round trip. It proves nothing but that the
/// store hands back what it was given, so it is random and never reused.
/// `None` when the operating system has no randomness to give, which is a
/// probe that cannot be created.
fn probe_secret() -> Option<Vec<u8>> {
	let mut secret = vec![0u8; 32];
	getrandom::fill(&mut secret).ok()?;
	Some(secret)
}
