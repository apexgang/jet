use pretty_assertions::assert_eq;

use super::{ArtifactError, ArtifactVerifier, Sha256Digest};

#[test]
fn completion_requires_declared_size_and_sha256() {
	let digest = Sha256Digest::parse(
		"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
	)
	.unwrap();
	let mut verified = ArtifactVerifier::new(3, digest);
	verified.accept(b"a").unwrap();
	verified.accept(b"bc").unwrap();
	assert_eq!(verified.finish().unwrap(), digest);

	let mut truncated = ArtifactVerifier::new(4, digest);
	truncated.accept(b"abc").unwrap();
	assert_eq!(
		truncated.finish().unwrap_err(),
		ArtifactError::SizeMismatch {
			declared: 4,
			actual: 3,
		}
	);

	let wrong = Sha256Digest::parse(
		"0000000000000000000000000000000000000000000000000000000000000000",
	)
	.unwrap();
	let mut corrupt = ArtifactVerifier::new(3, wrong);
	corrupt.accept(b"abc").unwrap();
	assert_eq!(
		corrupt.finish().unwrap_err(),
		ArtifactError::HashMismatch {
			expected: wrong,
			actual: digest,
		}
	);
}

#[test]
fn digests_require_canonical_lowercase_hex() {
	let uppercase = Sha256Digest::parse(
		"BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
	);
	let short = Sha256Digest::parse("ba78");

	assert!(uppercase.is_err());
	assert!(short.is_err());
}
