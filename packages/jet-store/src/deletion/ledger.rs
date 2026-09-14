//! The ledger's chain: what a record is, what the head vouches for, and
//! how a deletion is appended so that it folds through the head
//! (ADR-0102). The chain itself is the shared [`evidence`](crate::evidence)
//! shape; this module names the files and spells the records.

use crate::{
	StoreError,
	deletion::PendingDeletion,
	evidence::{self, Ledger, Reading},
};
use sha2::Digest as _;
use std::path::Path;
use uuid::Uuid;

/// What kind of identity a deletion removed. The ledger names the kind so
/// a restoration knows which row to remove again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletedIdentityKind {
	/// An Account binding that was unbound.
	AccountBinding,
	/// A Paired client that was revoked, with its key.
	PairedClient,
	/// A schedule that was cancelled.
	Schedule,
	/// A Conversation whose Jet Trash grace period ended (ADR-0015).
	Conversation,
	/// A Project that was removed (ADR-0011).
	Project,
}

impl DeletedIdentityKind {
	/// The stable spelling the ledger stores.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::AccountBinding => "account_binding",
			Self::PairedClient => "paired_client",
			Self::Schedule => "schedule",
			Self::Conversation => "conversation",
			Self::Project => "project",
		}
	}

	pub(super) fn parse(text: &str) -> Option<Self> {
		match text {
			"account_binding" => Some(Self::AccountBinding),
			"paired_client" => Some(Self::PairedClient),
			"schedule" => Some(Self::Schedule),
			"conversation" => Some(Self::Conversation),
			"project" => Some(Self::Project),
			_ => None,
		}
	}
}

/// One permanent deletion the ledger vouches for. It carries the identity
/// and the time, and nothing of what was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionRecord {
	/// Its position in the ledger, from one.
	pub sequence: u64,
	/// When the deletion was acknowledged, in Unix milliseconds.
	pub deleted_at_unix_ms: i64,
	/// What kind of identity was deleted.
	pub kind: DeletedIdentityKind,
	/// Which one.
	pub identity: Uuid,
}

/// The Deletion ledger as one instance of the evidence chain.
pub(crate) struct Deletions;

impl Ledger for Deletions {
	type Record = DeletionRecord;

	const NAME: &'static str = "Deletion ledger";
	const NOUN: &'static str = "deletion";
	const PLURAL: &'static str = "deletions";
	const LEDGER_SUFFIX: &'static str = ".deletions";
	const HEAD_SUFFIX: &'static str = ".deletions.head";
	const PENDING_HEAD_SUFFIX: &'static str = ".deletions.head.pending";
	const LEDGER_FORMAT: &'static str = "jet-deletion-ledger 1";
	const HEAD_FORMAT: &'static str = "jet-deletion-ledger-head 1";
	const GENESIS_DOMAIN: &'static [u8] = b"jet-deletion-ledger-genesis-v1";
	const RECORD_DOMAIN: &'static [u8] = b"jet-deletion-ledger-record-v1";

	fn sequence(record: &DeletionRecord) -> u64 {
		record.sequence
	}

	fn fold(record: &DeletionRecord, hasher: &mut sha2::Sha256) {
		hasher.update(record.deleted_at_unix_ms.to_be_bytes());
		let kind = record.kind.as_str().as_bytes();
		hasher.update(
			u64::try_from(kind.len()).unwrap_or(u64::MAX).to_be_bytes(),
		);
		hasher.update(kind);
		hasher.update(record.identity.as_bytes());
	}

	fn spell(record: &DeletionRecord) -> String {
		format!(
			"{} {} {}",
			record.deleted_at_unix_ms,
			record.kind.as_str(),
			record.identity
		)
	}

	fn parse(sequence: u64, fields: &[&str]) -> Option<DeletionRecord> {
		let [deleted_at, kind, identity] = fields else {
			return None;
		};
		Some(DeletionRecord {
			sequence,
			deleted_at_unix_ms: deleted_at.parse().ok()?,
			kind: DeletedIdentityKind::parse(kind)?,
			identity: Uuid::parse_str(identity).ok()?,
		})
	}
}

/// What reading the ledger found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionLedger {
	/// Every deletion folds through the head, oldest first. Empty when no
	/// deletion has been recorded on this Plane.
	Verified(Vec<DeletionRecord>),
	/// The ledger or its head is missing or altered, so the ledger vouches
	/// for nothing; `detail` says what was found.
	Corrupt(String),
}

impl DeletionLedger {
	/// The records, or the refusal every write that depends on the ledger
	/// meets when it vouches for nothing.
	///
	/// # Errors
	///
	/// Returns [`StoreError::Integrity`] naming what was found.
	pub(crate) fn trusted(self) -> Result<Vec<DeletionRecord>, StoreError> {
		match self {
			Self::Verified(records) => Ok(records),
			Self::Corrupt(detail) => {
				Err(evidence::untrusted::<Deletions>(&detail))
			}
		}
	}
}

/// Reads and verifies the ledger of the store at `database`; see
/// [`evidence::read`] for `plane_id` and `applied`.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] when a file cannot be read. A file
/// that reads but does not verify is reported as
/// [`DeletionLedger::Corrupt`], not as an error.
pub(crate) fn read(
	database: &Path,
	plane_id: Uuid,
	applied: u64,
) -> Result<DeletionLedger, StoreError> {
	Ok(
		match evidence::read::<Deletions>(database, plane_id, applied)? {
			Reading::Verified(records) => DeletionLedger::Verified(records),
			Reading::Corrupt(detail) => DeletionLedger::Corrupt(detail),
		},
	)
}

/// Appends `deletions` to the ledger durably, advances its head, and
/// returns the sequence the head now names, which the store records as
/// applied in the commit that follows; see [`evidence::append`].
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when the ledger cannot be trusted,
/// because a deletion cannot be chained to a ledger that vouches for
/// nothing, and [`StoreError::Unavailable`] when the files cannot be
/// written.
pub(crate) fn append(
	database: &Path,
	plane_id: Uuid,
	applied: u64,
	deletions: &[PendingDeletion],
) -> Result<u64, StoreError> {
	evidence::append::<Deletions, _>(
		database,
		plane_id,
		applied,
		deletions,
		|sequence, deletion| DeletionRecord {
			sequence,
			deleted_at_unix_ms: deletion.deleted_at_unix_ms,
			kind: deletion.kind,
			identity: deletion.identity,
		},
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::evidence::{evidence_path, recovery_dir};
	use pretty_assertions::assert_eq;
	use std::fs;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn deletion(kind: DeletedIdentityKind, at: i64) -> PendingDeletion {
		PendingDeletion {
			kind,
			identity: Uuid::now_v7(),
			deleted_at_unix_ms: at,
		}
	}

	fn record(sequence: u64, deletion: PendingDeletion) -> DeletionRecord {
		DeletionRecord {
			sequence,
			deleted_at_unix_ms: deletion.deleted_at_unix_ms,
			kind: deletion.kind,
			identity: deletion.identity,
		}
	}

	/// Appends chain across calls, the files are owner-only, and reading
	/// back gives every record in order, whether or not the reader knows
	/// its Plane.
	#[test]
	fn appended_deletions_are_read_back_in_order() {
		use std::os::unix::fs::PermissionsExt as _;
		let dir = tempfile::tempdir().unwrap();
		let database = dir.path().join("plane.sqlite3");
		let plane_id = Uuid::now_v7();
		let first = deletion(DeletedIdentityKind::PairedClient, NOW_UNIX_MS);
		let second =
			deletion(DeletedIdentityKind::AccountBinding, NOW_UNIX_MS + 1);
		let third = deletion(DeletedIdentityKind::Schedule, NOW_UNIX_MS + 2);

		append(&database, plane_id, 0, &[first, second]).unwrap();
		append(&database, plane_id, 0, &[third]).unwrap();

		let mode = |suffix| {
			fs::metadata(evidence_path(&database, suffix))
				.unwrap()
				.permissions()
				.mode() & 0o777
		};
		assert_eq!(
			(
				read(&database, plane_id, 0).unwrap(),
				// A store that cannot say which Plane it is reads the same
				// ledger.
				read(&database, Uuid::nil(), 0).unwrap(),
				mode(Deletions::LEDGER_SUFFIX),
				mode(Deletions::HEAD_SUFFIX),
				fs::metadata(recovery_dir(&database))
					.unwrap()
					.permissions()
					.mode() & 0o777,
			),
			(
				DeletionLedger::Verified(vec![
					record(1, first),
					record(2, second),
					record(3, third),
				]),
				DeletionLedger::Verified(vec![
					record(1, first),
					record(2, second),
					record(3, third),
				]),
				0o600,
				0o600,
				0o700,
			)
		);
	}

	/// No files is a Plane that has deleted nothing; a ledger of another
	/// Plane, an edited line, a shortened ledger, and a head with nothing
	/// under it are all corruption, each named.
	#[test]
	fn a_ledger_that_does_not_fold_through_its_head_vouches_for_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let database = dir.path().join("plane.sqlite3");
		let plane_id = Uuid::now_v7();
		let empty = read(&database, plane_id, 0).unwrap();
		append(
			&database,
			plane_id,
			0,
			&[
				deletion(DeletedIdentityKind::PairedClient, NOW_UNIX_MS),
				deletion(DeletedIdentityKind::Schedule, NOW_UNIX_MS + 1),
			],
		)
		.unwrap();
		let ledger = evidence_path(&database, Deletions::LEDGER_SUFFIX);
		let original = fs::read_to_string(&ledger).unwrap();
		let other = Uuid::now_v7();
		let other_plane = read(&database, other, 0).unwrap();

		let mut altered: Vec<String> =
			original.lines().map(ToOwned::to_owned).collect();
		altered[2] = altered[2].replacen("paired_client", "schedule", 1);
		fs::write(&ledger, altered.join("\n") + "\n").unwrap();
		let edited = read(&database, plane_id, 0).unwrap();

		let shortened: Vec<&str> = original.lines().take(3).collect();
		fs::write(&ledger, shortened.join("\n") + "\n").unwrap();
		let cut = read(&database, plane_id, 0).unwrap();

		fs::remove_file(&ledger).unwrap();
		let headless_ledger = read(&database, plane_id, 0).unwrap();

		fs::write(&ledger, &original).unwrap();
		fs::remove_file(evidence_path(&database, Deletions::HEAD_SUFFIX))
			.unwrap();
		let ledger_without_head = read(&database, plane_id, 0).unwrap();

		assert_eq!(
			(
				empty,
				other_plane,
				edited,
				cut,
				headless_ledger,
				ledger_without_head
			),
			(
				DeletionLedger::Verified(vec![]),
				DeletionLedger::Corrupt(format!(
					"it belongs to Plane {plane_id}, not {other}"
				)),
				DeletionLedger::Corrupt("deletion 1 was altered".into()),
				DeletionLedger::Corrupt(
					"its head names deletion 2, but the ledger ends at 1"
						.into()
				),
				DeletionLedger::Corrupt(
					"its head names deletion 2, but the ledger is missing"
						.into()
				),
				DeletionLedger::Corrupt(
					"it holds 2 deletions, but its head is missing".into()
				),
			)
		);
	}

	/// A line the head never confirmed is the trace of a crash between the
	/// ledger and its head. It is not vouched for, and the next append
	/// writes over it.
	#[test]
	fn a_line_past_the_head_is_discarded_by_the_next_append() {
		let dir = tempfile::tempdir().unwrap();
		let database = dir.path().join("plane.sqlite3");
		let plane_id = Uuid::now_v7();
		let first = deletion(DeletedIdentityKind::PairedClient, NOW_UNIX_MS);
		append(&database, plane_id, 0, &[first]).unwrap();
		let head =
			fs::read(evidence_path(&database, Deletions::HEAD_SUFFIX)).unwrap();
		let lost = deletion(DeletedIdentityKind::Schedule, NOW_UNIX_MS + 1);
		append(&database, plane_id, 0, &[lost]).unwrap();
		fs::write(evidence_path(&database, Deletions::HEAD_SUFFIX), head)
			.unwrap();
		let unconfirmed = read(&database, plane_id, 0).unwrap();

		let next =
			deletion(DeletedIdentityKind::AccountBinding, NOW_UNIX_MS + 2);
		append(&database, plane_id, 0, &[next]).unwrap();

		assert_eq!(
			(unconfirmed, read(&database, plane_id, 0).unwrap()),
			(
				DeletionLedger::Verified(vec![record(1, first)]),
				DeletionLedger::Verified(vec![
					record(1, first),
					record(2, next)
				]),
			)
		);
	}
}
