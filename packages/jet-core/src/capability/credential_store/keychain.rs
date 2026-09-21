//! The macOS Keychain, called through the Security framework (ADR-0076).
//!
//! Keychain Services opens the unlock dialog itself when a call needs one,
//! so every call here runs with user interaction disabled: a call that
//! would have asked fails with `errSecInteractionNotAllowed` instead, and
//! that answer is what tells a locked keychain from an open one. The
//! setting is process-wide, so calls are serialized and the setting is
//! restored as each one ends; any other Keychain use this process ever
//! makes must take the same lock, or it may find dialogs disabled.
//!
//! The lock state is read by asking the default keychain to unlock without
//! a password, which is a no-op on an open keychain and a refused dialog
//! on a locked one. `SecKeychainGetStatus` would read it directly, but the
//! Security framework crate does not bind it and this workspace forbids
//! unsafe code, so the refusal is the reading.

use super::{KIND, PROBE_ACCOUNT, PROBE_SERVICE, STORE_TIMEOUT, probe_secret};
use crate::capability::{
	CredentialProbeStep, CredentialStoreStatus, CredentialStoreVerification,
};
use security_framework::{
	base::Error,
	os::macos::keychain::{KeychainUserInteractionLock, SecKeychain},
	passwords,
};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// `errSecInteractionNotAllowed`: the call needed the user and was not
/// allowed to ask.
const INTERACTION_NOT_ALLOWED: i32 = -25308;
/// One Keychain call at a time, so the interaction one call disables is
/// not restored underneath another.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

pub(super) async fn observe() -> CredentialStoreStatus {
	blocking(CredentialStoreStatus::Unavailable { kind: KIND }, || {
		let Some(_quiet) = silence() else {
			return CredentialStoreStatus::Unavailable { kind: KIND };
		};
		match open_default() {
			Ok(()) => CredentialStoreStatus::Available { kind: KIND },
			Err(error) if needed_the_user(error) => {
				CredentialStoreStatus::Locked { kind: KIND }
			}
			Err(_) => CredentialStoreStatus::Unavailable { kind: KIND },
		}
	})
	.await
}

pub(super) async fn verify() -> CredentialStoreVerification {
	blocking(
		CredentialStoreVerification::Unavailable { kind: KIND },
		|| {
			let Some(_quiet) = silence() else {
				return CredentialStoreVerification::Unavailable { kind: KIND };
			};
			round_trip()
		},
	)
	.await
}

/// Asks the default keychain to be unlocked without a password. An open
/// keychain answers at once; a locked one would ask the user and, with
/// interaction disabled, says so instead.
fn open_default() -> Result<(), Error> {
	SecKeychain::default()?.unlock(None)
}

fn round_trip() -> CredentialStoreVerification {
	let Some(secret) = probe_secret() else {
		return CredentialStoreVerification::Failed {
			kind: KIND,
			step: CredentialProbeStep::Create,
		};
	};
	match open_default() {
		Ok(()) => {}
		Err(error) if needed_the_user(error) => {
			return CredentialStoreVerification::Locked { kind: KIND };
		}
		Err(_) => {
			return CredentialStoreVerification::Unavailable { kind: KIND };
		}
	}
	match passwords::set_generic_password(PROBE_SERVICE, PROBE_ACCOUNT, &secret)
	{
		Ok(()) => {}
		Err(error) if needed_the_user(error) => {
			return CredentialStoreVerification::Locked { kind: KIND };
		}
		Err(_) => {
			return CredentialStoreVerification::Failed {
				kind: KIND,
				step: CredentialProbeStep::Create,
			};
		}
	}
	let read = passwords::get_generic_password(PROBE_SERVICE, PROBE_ACCOUNT);
	// The item is deleted whatever the read said: a probe never outlives
	// its round trip.
	let deleted =
		passwords::delete_generic_password(PROBE_SERVICE, PROBE_ACCOUNT);
	if !matches!(&read, Ok(value) if *value == secret) {
		return CredentialStoreVerification::Failed {
			kind: KIND,
			step: CredentialProbeStep::Read,
		};
	}
	match deleted {
		Ok(()) => CredentialStoreVerification::Verified { kind: KIND },
		Err(_) => CredentialStoreVerification::Failed {
			kind: KIND,
			step: CredentialProbeStep::Delete,
		},
	}
}

/// Whether the framework refused a call because it would have had to
/// ask the user, which is what a locked keychain looks like from here.
fn needed_the_user(error: Error) -> bool {
	error.code() == INTERACTION_NOT_ALLOWED
}

/// Takes the Keychain for this thread and switches its dialogs off for as
/// long as the guard lives. `None` when the framework refused to switch
/// them off, in which case no call is made: a call that might ask the
/// user is worse than no answer.
fn silence() -> Option<(MutexGuard<'static, ()>, KeychainUserInteractionLock)> {
	let one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
	let quiet = SecKeychain::disable_user_interaction().ok()?;
	Some((one, quiet))
}

/// Runs one Keychain conversation off the async runtime; the framework
/// blocks, and a blocked runtime thread would stall the Plane. A
/// conversation that outlasts [`STORE_TIMEOUT`] answers `fallback`, and
/// the thread it holds finishes on its own.
async fn blocking<T: Send + 'static>(
	fallback: T,
	work: impl FnOnce() -> T + Send + 'static,
) -> T {
	match tokio::time::timeout(STORE_TIMEOUT, tokio::task::spawn_blocking(work))
		.await
	{
		Ok(Ok(answer)) => answer,
		Ok(Err(_)) | Err(_) => fallback,
	}
}
