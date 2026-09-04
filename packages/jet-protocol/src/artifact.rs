//! Incremental integrity verification for Artifact streams.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

/// Exactly one 256-bit SHA-256 digest, serialized as lowercase hexadecimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
	/// Parses the canonical 64-character lowercase hexadecimal form.
	///
	/// # Errors
	///
	/// Returns [`DigestError`] for the wrong length, uppercase, or non-hex
	/// input.
	pub fn parse(value: &str) -> Result<Self, DigestError> {
		if value.len() != 64 {
			return Err(DigestError);
		}
		let mut bytes = [0u8; 32];
		let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
		debug_assert!(remainder.is_empty());
		for (index, pair) in pairs.iter().enumerate() {
			bytes[index] = (hex_nibble(pair[0]).ok_or(DigestError)? << 4)
				| hex_nibble(pair[1]).ok_or(DigestError)?;
		}
		Ok(Self(bytes))
	}
}

impl fmt::Display for Sha256Digest {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		for byte in self.0 {
			write!(formatter, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl Serialize for Sha256Digest {
	fn serialize<S: Serializer>(
		&self,
		serializer: S,
	) -> Result<S::Ok, S::Error> {
		serializer.collect_str(self)
	}
}

impl<'de> Deserialize<'de> for Sha256Digest {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		let value = <&str>::deserialize(deserializer)?;
		Self::parse(value).map_err(serde::de::Error::custom)
	}
}

/// A value was not a canonical SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("SHA-256 must be 64 lowercase hexadecimal characters")]
pub struct DigestError;

fn hex_nibble(byte: u8) -> Option<u8> {
	match byte {
		b'0'..=b'9' => Some(byte - b'0'),
		b'a'..=b'f' => Some(byte - b'a' + 10),
		_ => None,
	}
}

/// Incremental receiver-side Artifact size and SHA-256 verification.
#[derive(Debug)]
pub struct ArtifactVerifier {
	declared_size: u64,
	expected: Sha256Digest,
	received: u64,
	hasher: Sha256,
}

impl ArtifactVerifier {
	/// Starts verifying a stream against its declaration.
	#[must_use]
	pub fn new(declared_size: u64, expected: Sha256Digest) -> Self {
		Self {
			declared_size,
			expected,
			received: 0,
			hasher: Sha256::new(),
		}
	}

	/// Accepts one bounded data chunk without retaining it.
	///
	/// # Errors
	///
	/// Rejects a chunk that would exceed the declared size.
	pub fn accept(&mut self, chunk: &[u8]) -> Result<(), ArtifactError> {
		let received = self
			.received
			.checked_add(u64::try_from(chunk.len()).map_err(|_| {
				ArtifactError::SizeExceeded {
					declared: self.declared_size,
					attempted: u64::MAX,
				}
			})?)
			.ok_or(ArtifactError::SizeExceeded {
				declared: self.declared_size,
				attempted: u64::MAX,
			})?;
		if received > self.declared_size {
			return Err(ArtifactError::SizeExceeded {
				declared: self.declared_size,
				attempted: received,
			});
		}
		self.received = received;
		self.hasher.update(chunk);
		Ok(())
	}

	/// Completes verification only when size and SHA-256 both match.
	///
	/// # Errors
	///
	/// Returns the independently observed size or hash mismatch.
	pub fn finish(self) -> Result<Sha256Digest, ArtifactError> {
		if self.received != self.declared_size {
			return Err(ArtifactError::SizeMismatch {
				declared: self.declared_size,
				actual: self.received,
			});
		}
		// ASVS 11.4.1 and 11.4.3: Artifact integrity uses the
		// collision-resistant 256-bit hash required by ADR-0091.
		let actual = Sha256Digest(self.hasher.finalize().into());
		if actual != self.expected {
			return Err(ArtifactError::HashMismatch {
				expected: self.expected,
				actual,
			});
		}
		Ok(actual)
	}
}

/// Artifact completion failed its declared size or integrity contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArtifactError {
	/// A chunk would exceed the declared Artifact size.
	#[error(
		"Artifact declared {declared} bytes but received at least {attempted}"
	)]
	SizeExceeded {
		/// Size declared before streaming.
		declared: u64,
		/// Size after accepting the offending chunk.
		attempted: u64,
	},
	/// The stream ended before reaching its exact declaration.
	#[error("Artifact declared {declared} bytes but completed with {actual}")]
	SizeMismatch {
		/// Size declared before streaming.
		declared: u64,
		/// Bytes actually received.
		actual: u64,
	},
	/// The complete content did not match its declared SHA-256.
	#[error("Artifact SHA-256 did not match its declaration")]
	HashMismatch {
		/// Digest declared before streaming.
		expected: Sha256Digest,
		/// Digest computed across received bytes.
		actual: Sha256Digest,
	},
}

#[cfg(test)]
#[path = "artifact_tests.rs"]
mod tests;
