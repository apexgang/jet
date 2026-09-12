//! The ledger's chain: what a record is, what the head vouches for, and
//! how a deletion is appended so that it folds through the head
//! (ADR-0102). The files themselves live in [`files`](super::files).

use crate::{
	StoreError,
	deletion::{
		PendingDeletion,
		files::{self, Head, Link},
	},
};
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
}

impl DeletedIdentityKind {
	/// The stable spelling the ledger stores.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::AccountBinding => "account_binding",
			Self::PairedClient => "paired_client",
			Self::Schedule => "schedule",
		}
	}

	pub(super) fn parse(text: &str) -> Option<Self> {
		match text {
			"account_binding" => Some(Self::AccountBinding),
			"paired_client" => Some(Self::PairedClient),
			"schedule" => Some(Self::Schedule),
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
			Self::Corrupt(detail) => Err(untrusted(&detail)),
		}
	}
}

/// The head's view of the ledger: what it vouches for.
struct Vouched {
	/// The records the head reaches, with their links.
	records: Vec<(DeletionRecord, Link)>,
	/// Where the vouched-for part of the ledger ends in the file, or
	/// `None` when there is no file yet.
	end: Option<u64>,
}

/// Reads and verifies the ledger of the store at `database`.
///
/// `plane_id` is the Plane the ledger must belong to, or nil when the
/// store is too damaged to say; see [`check_plane`]. `applied` is how
/// many records the store says it has applied: a ledger that vouches for
/// fewer is evidence gone missing, files and all.
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
	match vouched(database, plane_id).and_then(|vouched| {
		behind_store(vouched.records.len(), applied)?;
		Ok(vouched)
	}) {
		Ok(vouched) => Ok(DeletionLedger::Verified(
			vouched
				.records
				.into_iter()
				.map(|(record, _)| record)
				.collect(),
		)),
		Err(StoreError::Integrity(detail)) => {
			Ok(DeletionLedger::Corrupt(detail))
		}
		Err(error) => Err(error),
	}
}

/// Whether a ledger that vouches for `vouched` records is behind a store
/// that has applied `applied` of them, which no crash ordering produces.
fn behind_store(vouched: usize, applied: u64) -> Result<(), StoreError> {
	let vouched = u64::try_from(vouched).unwrap_or(u64::MAX);
	if vouched < applied {
		return Err(StoreError::Integrity(format!(
			"the store has applied {applied} deletions, but the ledger \
			 vouches for {vouched}"
		)));
	}
	Ok(())
}

/// Appends `deletions` to the ledger durably, advances its head, and
/// returns the sequence the head now names, which the store records as
/// applied in the commit that follows. `applied` is what the store has
/// recorded so far; see [`read`].
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
	let Vouched { records, end } = vouched(database, plane_id)
		.and_then(|vouched| {
			behind_store(vouched.records.len(), applied)?;
			Ok(vouched)
		})
		.map_err(|error| match error {
			StoreError::Integrity(detail) => untrusted(&detail),
			other => other,
		})?;
	let mut head = records.last().map_or(
		Head {
			sequence: 0,
			link: Link::genesis(plane_id),
		},
		|(record, link)| Head {
			sequence: record.sequence,
			link: *link,
		},
	);
	let mut body = String::new();
	for deletion in deletions {
		let record = DeletionRecord {
			sequence: head.sequence + 1,
			deleted_at_unix_ms: deletion.deleted_at_unix_ms,
			kind: deletion.kind,
			identity: deletion.identity,
		};
		head = Head {
			sequence: record.sequence,
			link: Link::after(head.link, &record),
		};
		body.push_str(&format!(
			"{} {} {} {} {}\n",
			record.sequence,
			record.deleted_at_unix_ms,
			record.kind.as_str(),
			record.identity,
			head.link
		));
	}
	files::write_lines(database, plane_id, end, &body)?;
	files::write_head(database, plane_id, head)?;
	Ok(head.sequence)
}

/// The records the head vouches for and where they end in the file.
fn vouched(database: &Path, plane_id: Uuid) -> Result<Vouched, StoreError> {
	let head = files::read_head(database, plane_id)?;
	let file = files::read_ledger_file(database, plane_id)?;
	match (head, file) {
		(None, None) => Ok(Vouched {
			records: vec![],
			end: None,
		}),
		(Some(head), None) => Err(StoreError::Integrity(format!(
			"its head names deletion {}, but the ledger is missing",
			head.sequence
		))),
		// A single line past a missing head is the first deletion, written
		// just before the crash that kept its head from following.
		(None, Some(file)) => match file.records.len() {
			0 | 1 => Ok(Vouched {
				records: vec![],
				end: Some(file.header_end),
			}),
			count => Err(StoreError::Integrity(format!(
				"it holds {count} deletions, but its head is missing"
			))),
		},
		(Some(head), Some(file)) => {
			let sequence = head.sequence;
			let vouched = usize::try_from(sequence).unwrap_or(usize::MAX);
			let count = file.records.len();
			if count < vouched {
				return Err(StoreError::Integrity(format!(
					"its head names deletion {sequence}, but the ledger ends \
					 at {count}"
				)));
			}
			if count > vouched.saturating_add(1) {
				return Err(StoreError::Integrity(format!(
					"its head names deletion {sequence}, but the ledger goes \
					 on to {count}"
				)));
			}
			// The head never names sequence zero; `read_head` refuses it.
			let newest = vouched.saturating_sub(1);
			if file.records[newest].1 != head.link {
				return Err(StoreError::Integrity(format!(
					"deletion {sequence} is not the one its head names"
				)));
			}
			let mut records = file.records;
			records.truncate(vouched);
			Ok(Vouched {
				end: Some(file.ends[newest]),
				records,
			})
		}
	}
}

/// The refusal a ledger that vouches for nothing gives every write that
/// depends on it.
fn untrusted(detail: &str) -> StoreError {
	StoreError::Integrity(format!(
		"the Deletion ledger cannot be trusted: {detail}"
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::deletion::files::{
		HEAD_SUFFIX, LEDGER_SUFFIX, evidence_path, recovery_dir,
	};
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
				mode(LEDGER_SUFFIX),
				mode(HEAD_SUFFIX),
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
		let ledger = evidence_path(&database, LEDGER_SUFFIX);
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
		fs::remove_file(evidence_path(&database, HEAD_SUFFIX)).unwrap();
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
		let head = fs::read(evidence_path(&database, HEAD_SUFFIX)).unwrap();
		let lost = deletion(DeletedIdentityKind::Schedule, NOW_UNIX_MS + 1);
		append(&database, plane_id, 0, &[lost]).unwrap();
		fs::write(evidence_path(&database, HEAD_SUFFIX), head).unwrap();
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
