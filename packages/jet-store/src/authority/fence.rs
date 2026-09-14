//! The Authority fence ledger: which Conversation authorities this Plane
//! has retired, so that neither the live store nor a restored snapshot
//! can continue one (ADR-0070). The chain is the shared
//! [`evidence`](crate::evidence) shape; this module names the files and
//! spells the records.

use crate::{
	StoreError,
	evidence::{self, Ledger, Reading},
};
use sha2::Digest as _;
use std::path::Path;
use uuid::Uuid;

/// One retired Conversation authority the fences vouch for. It names the
/// Conversation, the epoch that ended, the transfer that ended it, and
/// the Plane the authority went to, and nothing of the Conversation's
/// content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityFenceRecord {
	/// Its position in the ledger, from one.
	pub sequence: u64,
	/// When the source relinquished, in Unix milliseconds.
	pub fenced_at_unix_ms: i64,
	/// The Conversation whose authority was retired.
	pub conversation_id: Uuid,
	/// The authority epoch this Plane held and retired.
	pub retired_epoch: u64,
	/// The Plane transfer that retired it.
	pub transfer_id: Uuid,
	/// The Plane that holds the next epoch.
	pub target_plane_id: Uuid,
}

/// A fence a write transaction has raised and the ledger has not yet
/// recorded. The transaction collects them and the store writes them
/// just before it commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingFence {
	/// When the source relinquished, in Unix milliseconds.
	pub fenced_at_unix_ms: i64,
	/// The Conversation whose authority is retired.
	pub conversation_id: Uuid,
	/// The authority epoch this Plane retires.
	pub retired_epoch: u64,
	/// The Plane transfer retiring it.
	pub transfer_id: Uuid,
	/// The Plane that takes the next epoch.
	pub target_plane_id: Uuid,
}

/// What reading the fences found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityFences {
	/// Every fence folds through the head, oldest first. Empty when this
	/// Plane has retired no authority.
	Verified(Vec<AuthorityFenceRecord>),
	/// The ledger or its head is missing or altered, so it vouches for
	/// nothing; `detail` says what was found.
	Corrupt(String),
}

impl AuthorityFences {
	/// The records, or the refusal every write that depends on the fences
	/// meets when they vouch for nothing.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] naming what was found.
	pub(crate) fn trusted(
		self,
	) -> Result<Vec<AuthorityFenceRecord>, StoreError> {
		match self {
			Self::Verified(records) => Ok(records),
			Self::Corrupt(detail) => {
				Err(evidence::untrusted::<Fences>(&detail))
			}
		}
	}
}

/// The Authority fences as one instance of the evidence chain.
pub(crate) struct Fences;

impl Ledger for Fences {
	type Record = AuthorityFenceRecord;

	const NAME: &'static str = "Authority fences";
	const NOUN: &'static str = "fence";
	const PLURAL: &'static str = "fences";
	const LEDGER_SUFFIX: &'static str = ".fences";
	const HEAD_SUFFIX: &'static str = ".fences.head";
	const PENDING_HEAD_SUFFIX: &'static str = ".fences.head.pending";
	const LEDGER_FORMAT: &'static str = "jet-authority-fences 1";
	const HEAD_FORMAT: &'static str = "jet-authority-fences-head 1";
	const GENESIS_DOMAIN: &'static [u8] = b"jet-authority-fence-genesis-v1";
	const RECORD_DOMAIN: &'static [u8] = b"jet-authority-fence-record-v1";

	fn sequence(record: &AuthorityFenceRecord) -> u64 {
		record.sequence
	}

	fn fold(record: &AuthorityFenceRecord, hasher: &mut sha2::Sha256) {
		hasher.update(record.fenced_at_unix_ms.to_be_bytes());
		hasher.update(record.conversation_id.as_bytes());
		hasher.update(record.retired_epoch.to_be_bytes());
		hasher.update(record.transfer_id.as_bytes());
		hasher.update(record.target_plane_id.as_bytes());
	}

	fn spell(record: &AuthorityFenceRecord) -> String {
		format!(
			"{} {} {} {} {}",
			record.fenced_at_unix_ms,
			record.conversation_id,
			record.retired_epoch,
			record.transfer_id,
			record.target_plane_id
		)
	}

	fn parse(sequence: u64, fields: &[&str]) -> Option<AuthorityFenceRecord> {
		let [fenced_at, conversation, epoch, transfer, target] = fields else {
			return None;
		};
		Some(AuthorityFenceRecord {
			sequence,
			fenced_at_unix_ms: fenced_at.parse().ok()?,
			conversation_id: Uuid::parse_str(conversation).ok()?,
			retired_epoch: epoch.parse().ok()?,
			transfer_id: Uuid::parse_str(transfer).ok()?,
			target_plane_id: Uuid::parse_str(target).ok()?,
		})
	}
}

/// Reads and verifies the fences of the store at `database`; see
/// [`evidence::read`] for `plane_id` and `applied`.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] when a file cannot be read. A file
/// that reads but does not verify is reported as
/// [`AuthorityFences::Corrupt`], not as an error.
pub(crate) fn read(
	database: &Path,
	plane_id: Uuid,
	applied: u64,
) -> Result<AuthorityFences, StoreError> {
	Ok(
		match evidence::read::<Fences>(database, plane_id, applied)? {
			Reading::Verified(records) => AuthorityFences::Verified(records),
			Reading::Corrupt(detail) => AuthorityFences::Corrupt(detail),
		},
	)
}

/// Appends `fences` durably, advances the head, and returns the sequence
/// the head now names, which the store records as applied in the commit
/// that follows; see [`evidence::append`].
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when the fences cannot be trusted,
/// because a fence cannot be chained to a ledger that vouches for
/// nothing, and [`StoreError::Unavailable`] when the files cannot be
/// written.
pub(crate) fn append(
	database: &Path,
	plane_id: Uuid,
	applied: u64,
	fences: &[PendingFence],
) -> Result<u64, StoreError> {
	evidence::append::<Fences, _>(
		database,
		plane_id,
		applied,
		fences,
		|sequence, fence| AuthorityFenceRecord {
			sequence,
			fenced_at_unix_ms: fence.fenced_at_unix_ms,
			conversation_id: fence.conversation_id,
			retired_epoch: fence.retired_epoch,
			transfer_id: fence.transfer_id,
			target_plane_id: fence.target_plane_id,
		},
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::evidence::evidence_path;
	use pretty_assertions::assert_eq;
	use std::fs;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn fence(at: i64) -> PendingFence {
		PendingFence {
			fenced_at_unix_ms: at,
			conversation_id: Uuid::now_v7(),
			retired_epoch: 3,
			transfer_id: Uuid::now_v7(),
			target_plane_id: Uuid::now_v7(),
		}
	}

	fn record(sequence: u64, fence: PendingFence) -> AuthorityFenceRecord {
		AuthorityFenceRecord {
			sequence,
			fenced_at_unix_ms: fence.fenced_at_unix_ms,
			conversation_id: fence.conversation_id,
			retired_epoch: fence.retired_epoch,
			transfer_id: fence.transfer_id,
			target_plane_id: fence.target_plane_id,
		}
	}

	/// Fences read back in order through their own files, beside the
	/// Deletion ledger, and an edited fence is noticed by name.
	#[test]
	fn appended_fences_are_read_back_and_an_altered_one_is_noticed() {
		let dir = tempfile::tempdir().unwrap();
		let database = dir.path().join("plane.sqlite3");
		let plane_id = Uuid::now_v7();
		let first = fence(NOW_UNIX_MS);
		let second = fence(NOW_UNIX_MS + 1);
		append(&database, plane_id, 0, &[first]).unwrap();
		append(&database, plane_id, 1, &[second]).unwrap();
		let verified = read(&database, plane_id, 2).unwrap();

		let ledger = evidence_path(&database, Fences::LEDGER_SUFFIX);
		let mut lines: Vec<String> = fs::read_to_string(&ledger)
			.unwrap()
			.lines()
			.map(ToOwned::to_owned)
			.collect();
		lines[3] = lines[3].replacen(" 3 ", " 4 ", 1);
		fs::write(&ledger, lines.join("\n") + "\n").unwrap();

		assert_eq!(
			(
				verified,
				lines[0].clone(),
				read(&database, plane_id, 2).unwrap()
			),
			(
				AuthorityFences::Verified(vec![
					record(1, first),
					record(2, second)
				]),
				"jet-authority-fences 1".to_string(),
				AuthorityFences::Corrupt("fence 2 was altered".into()),
			)
		);
	}
}
