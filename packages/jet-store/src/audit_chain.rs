//! The hashes that chain the Security audit together (ADR-0105).
//!
//! Every record commits to the one before it, so altering a record after
//! the fact changes every hash that follows and no longer folds to the
//! durable head kept outside the database.
//!
//! Two properties shape the encoding. Each field is written with its own
//! length, so no two different records can produce the same bytes by moving
//! a separator; and a target enters the chain as an opaque reference rather
//! than as its own identity, so deleting what it names leaves the chain
//! intact (see [`target_reference`]).

use std::fmt;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::StoreError;

/// One link of the audit chain: SHA-256 over the previous link and the
/// durable fields of the record that follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditEntryHash(pub [u8; 32]);

/// The content-free identifier of one audit target. It is what the chain
/// covers, so a record keeps saying what it was about after the Plane has
/// forgotten the thing itself (ADR-0105).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditTargetRef(pub [u8; 32]);

/// The durable fields of one record, in the order the chain covers them.
pub(crate) struct ChainedFields<'a> {
	pub(crate) sequence: u64,
	pub(crate) epoch: u64,
	pub(crate) record_id: Uuid,
	pub(crate) recorded_at_unix_ms: i64,
	pub(crate) plane_id: Uuid,
	pub(crate) actor_kind: &'a str,
	pub(crate) actor_id: &'a str,
	pub(crate) target_kind: &'a str,
	pub(crate) target_reference: AuditTargetRef,
	pub(crate) decision: &'a str,
	pub(crate) risk: &'a str,
	pub(crate) outcome: &'a str,
}

/// What one epoch of the chain begins from: the epoch itself and, when it
/// succeeds another, the head that other epoch was last known to have.
pub(crate) struct EpochGenesis<'a> {
	pub(crate) epoch: u64,
	pub(crate) plane_id: Uuid,
	pub(crate) started_at_unix_ms: i64,
	pub(crate) preceding: Option<(u64, AuditEntryHash)>,
	pub(crate) gap_reason: Option<&'a str>,
}

/// Domain separators, so a hash of one kind can never be read as another.
const ENTRY_DOMAIN: &[u8] = b"jet.audit.entry.v1";
const EPOCH_DOMAIN: &[u8] = b"jet.audit.epoch.v1";
const TARGET_DOMAIN: &[u8] = b"jet.audit.target.v1";

/// The link that follows `previous` for a record with these fields.
pub(crate) fn entry_hash(
	previous: AuditEntryHash,
	fields: &ChainedFields<'_>,
) -> AuditEntryHash {
	let mut hasher = Sha256::new();
	hasher.update(ENTRY_DOMAIN);
	hasher.update(previous.0);
	hasher.update(fields.sequence.to_be_bytes());
	hasher.update(fields.epoch.to_be_bytes());
	field(&mut hasher, fields.record_id.as_bytes());
	hasher.update(fields.recorded_at_unix_ms.to_be_bytes());
	field(&mut hasher, fields.plane_id.as_bytes());
	field(&mut hasher, fields.actor_kind.as_bytes());
	field(&mut hasher, fields.actor_id.as_bytes());
	field(&mut hasher, fields.target_kind.as_bytes());
	hasher.update(fields.target_reference.0);
	field(&mut hasher, fields.decision.as_bytes());
	field(&mut hasher, fields.risk.as_bytes());
	field(&mut hasher, fields.outcome.as_bytes());
	AuditEntryHash(hasher.finalize().into())
}

/// The link the first record of `genesis` follows. It covers the gap the
/// epoch leaves behind, so a new epoch cannot quietly claim to continue the
/// one it replaced.
pub(crate) fn genesis_hash(genesis: &EpochGenesis<'_>) -> AuditEntryHash {
	let mut hasher = Sha256::new();
	hasher.update(EPOCH_DOMAIN);
	hasher.update(genesis.epoch.to_be_bytes());
	field(&mut hasher, genesis.plane_id.as_bytes());
	hasher.update(genesis.started_at_unix_ms.to_be_bytes());
	match genesis.preceding {
		None => {
			hasher.update(0_u64.to_be_bytes());
			hasher.update([0_u8; 32]);
		}
		Some((sequence, hash)) => {
			hasher.update(sequence.to_be_bytes());
			hasher.update(hash.0);
		}
	}
	field(
		&mut hasher,
		genesis.gap_reason.unwrap_or_default().as_bytes(),
	);
	AuditEntryHash(hasher.finalize().into())
}

/// The opaque reference for a target on `plane_id`.
///
/// It is derived rather than random so records about the same target still
/// group together after that target is gone, and it is one-way so the
/// record that survives says only that *some* Conversation was deleted.
pub(crate) fn target_reference(
	plane_id: Uuid,
	kind: &str,
	id: Option<&str>,
) -> AuditTargetRef {
	let mut hasher = Sha256::new();
	hasher.update(TARGET_DOMAIN);
	field(&mut hasher, plane_id.as_bytes());
	field(&mut hasher, kind.as_bytes());
	field(&mut hasher, id.unwrap_or_default().as_bytes());
	// A target with no identity of its own and one whose identity is the
	// empty string are the same reference; no caller has the second.
	AuditTargetRef(hasher.finalize().into())
}

/// Writes one length-prefixed field, so field boundaries cannot be moved.
fn field(hasher: &mut Sha256, bytes: &[u8]) {
	hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
	hasher.update(bytes);
}

impl AuditTargetRef {
	/// Reads a stored reference.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the column is not 32 bytes.
	pub(crate) fn from_column(bytes: &[u8]) -> Result<Self, StoreError> {
		thirty_two("an audit target reference", bytes).map(Self)
	}
}

impl AuditEntryHash {
	/// Reads a stored link.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when the column is not 32 bytes.
	pub(crate) fn from_column(bytes: &[u8]) -> Result<Self, StoreError> {
		thirty_two("an audit chain hash", bytes).map(Self)
	}

	/// Reads the lowercase hexadecimal spelling written by [`fmt::Display`].
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] when `text` is not exactly 32 bytes
	/// of hexadecimal.
	pub(crate) fn parse_hex(text: &str) -> Result<Self, StoreError> {
		let mut bytes = [0_u8; 32];
		if text.len() != 64 {
			return Err(StoreError::Integrity(format!(
				"an audit chain hash is 64 hexadecimal characters, not {}",
				text.len()
			)));
		}
		for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
			let pair = std::str::from_utf8(pair).map_err(|_| {
				StoreError::Integrity(
					"an audit chain hash is hexadecimal".into(),
				)
			})?;
			*byte = u8::from_str_radix(pair, 16).map_err(|error| {
				StoreError::Integrity(format!(
					"an audit chain hash is hexadecimal: {error}"
				))
			})?;
		}
		Ok(Self(bytes))
	}
}

impl fmt::Display for AuditEntryHash {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write_hex(formatter, self.0)
	}
}

impl fmt::Display for AuditTargetRef {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write_hex(formatter, self.0)
	}
}

/// The width every identifier here is stored at.
fn thirty_two(described: &str, bytes: &[u8]) -> Result<[u8; 32], StoreError> {
	<[u8; 32]>::try_from(bytes).map_err(|_| {
		StoreError::Integrity(format!(
			"{described} is 32 bytes, not {}",
			bytes.len()
		))
	})
}

/// The lowercase hexadecimal spelling both identifiers cross a seam in.
fn write_hex(
	formatter: &mut fmt::Formatter<'_>,
	bytes: [u8; 32],
) -> fmt::Result {
	for byte in bytes {
		write!(formatter, "{byte:02x}")?;
	}
	Ok(())
}
