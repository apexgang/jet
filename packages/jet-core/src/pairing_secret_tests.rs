use std::collections::HashSet;

use jet_store::PairingMethod;

use super::{digest, matches, salt};
use crate::pairing::PairingSecret;

#[test]
fn fresh_salts_separate_digests_of_the_same_secret() {
	let method = PairingMethod::ManualCode;
	let secret = PairingSecret("1234-5678".into());
	let mut salts = HashSet::new();
	let mut digests = HashSet::new();
	for _ in 0..32 {
		let salt = salt().expect("the OS supplies entropy");
		let hashed = digest(&salt, &method, &secret);
		// Report only the failed invariant, never salts, digests, or secrets.
		assert!(salt != [0; 16], "salt must be initialized with entropy");
		assert!(salts.insert(salt), "each offer needs a fresh salt");
		assert!(digests.insert(hashed), "salt must affect the secret digest");
		assert!(matches(&salt, &hashed, &method, &secret));
	}
}

#[test]
fn a_digest_is_bound_to_its_salt_and_normalized_secret() {
	let method = PairingMethod::ManualCode;
	let secret = PairingSecret("1234-5678".into());
	let original_salt = salt().expect("the OS supplies entropy");
	let other_salt = salt().expect("the OS supplies entropy");
	let hashed = digest(&original_salt, &method, &secret);

	assert!(matches(
		&original_salt,
		&hashed,
		&method,
		&PairingSecret("1234 5678".into()),
	));
	assert!(!matches(&other_salt, &hashed, &method, &secret));
	assert!(!matches(
		&original_salt,
		&hashed,
		&method,
		&PairingSecret("8765-4321".into()),
	));
}
