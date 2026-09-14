//! Chained evidence ledgers kept beside the database, outside SQLite and
//! its rollback-capable snapshots (ADR-0102).
//!
//! A ledger is an append-only text file of numbered records folded into
//! an integrity chain, with the chain's durable head in its own file. The
//! Deletion ledger and the Authority fences are two instances of one
//! shape; each names its files, its formats, its hash domains, and how
//! one record is spelled and folded, through [`Ledger`]. What the chain
//! provides is the same for both: every record the head vouches for, in
//! order, or the finding that the ledger vouches for nothing.

mod files;

use files::{Head, Link};
#[cfg(test)]
pub(crate) use files::{evidence_path, recovery_dir};
use sha2::Sha256;
use std::path::Path;
use uuid::Uuid;

use crate::StoreError;

/// One kind of chained evidence: the files it lives in and how its
/// records are spelled. Implementations are unit types; the records they
/// describe carry their own sequence, assigned as they are appended.
pub(crate) trait Ledger {
	/// One record of the ledger, with its sequence.
	type Record: Copy + PartialEq;

	/// How the ledger is named in refusals, such as `Deletion ledger`.
	const NAME: &'static str;
	/// What one record is called in refusals, such as `deletion`.
	const NOUN: &'static str;
	/// What several records are called, such as `deletions`.
	const PLURAL: &'static str;
	/// Suffixes appended to the database file name. Deriving them keeps
	/// two stores in one directory from sharing a ledger.
	const LEDGER_SUFFIX: &'static str;
	const HEAD_SUFFIX: &'static str;
	const PENDING_HEAD_SUFFIX: &'static str;
	/// First lines of the files, so a future format is recognized rather
	/// than misread.
	const LEDGER_FORMAT: &'static str;
	const HEAD_FORMAT: &'static str;
	/// Domain separators of the chain's genesis and record links.
	const GENESIS_DOMAIN: &'static [u8];
	const RECORD_DOMAIN: &'static [u8];

	/// The record's position in the ledger, from one.
	fn sequence(record: &Self::Record) -> u64;
	/// Folds every field of the record after its sequence into the link
	/// that follows it.
	fn fold(record: &Self::Record, hasher: &mut Sha256);
	/// The record's fields after its sequence, space-separated, as the
	/// line spells them before its link.
	fn spell(record: &Self::Record) -> String;
	/// Reads the fields after the sequence back, or `None` when the line
	/// is malformed. `fields` holds exactly what [`Self::spell`] wrote.
	fn parse(sequence: u64, fields: &[&str]) -> Option<Self::Record>;
}

/// What reading a ledger found: every record the head vouches for, oldest
/// first, or the finding that the ledger vouches for nothing.
pub(crate) enum Reading<R> {
	Verified(Vec<R>),
	Corrupt(String),
}

/// The head's view of the ledger: what it vouches for.
struct Vouched<R> {
	/// The records the head reaches, with their links.
	records: Vec<(R, Link)>,
	/// Where the vouched-for part of the ledger ends in the file, or
	/// `None` when there is no file yet.
	end: Option<u64>,
}

/// Reads and verifies the ledger of the store at `database`.
///
/// `plane_id` is the Plane the ledger must belong to, or nil when the
/// store is too damaged to say. `applied` is how many records the store
/// says it has applied: a ledger that vouches for fewer is evidence gone
/// missing, files and all.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] when a file cannot be read. A file
/// that reads but does not verify is reported as [`Reading::Corrupt`],
/// not as an error.
pub(crate) fn read<L: Ledger>(
	database: &Path,
	plane_id: Uuid,
	applied: u64,
) -> Result<Reading<L::Record>, StoreError> {
	match vouched::<L>(database, plane_id).and_then(|vouched| {
		behind_store::<L>(vouched.records.len(), applied)?;
		Ok(vouched)
	}) {
		Ok(vouched) => Ok(Reading::Verified(
			vouched
				.records
				.into_iter()
				.map(|(record, _)| record)
				.collect(),
		)),
		Err(StoreError::Integrity(detail)) => Ok(Reading::Corrupt(detail)),
		Err(error) => Err(error),
	}
}

/// Whether a ledger that vouches for `vouched` records is behind a store
/// that has applied `applied` of them, which no crash ordering produces.
fn behind_store<L: Ledger>(
	vouched: usize,
	applied: u64,
) -> Result<(), StoreError> {
	let vouched = u64::try_from(vouched).unwrap_or(u64::MAX);
	if vouched < applied {
		return Err(StoreError::Integrity(format!(
			"the store has applied {applied} {}, but the ledger vouches for \
			 {vouched}",
			L::PLURAL
		)));
	}
	Ok(())
}

/// Appends one record per item of `pending` durably, each built by
/// `record` from the sequence it receives, advances the head, and returns
/// the sequence the head now names, which the store records as applied in
/// the commit that follows. `applied` is what the store has recorded so
/// far; see [`read`].
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when the ledger cannot be trusted,
/// because a record cannot be chained to a ledger that vouches for
/// nothing, and [`StoreError::Unavailable`] when the files cannot be
/// written.
pub(crate) fn append<L: Ledger, P>(
	database: &Path,
	plane_id: Uuid,
	applied: u64,
	pending: &[P],
	record: impl Fn(u64, &P) -> L::Record,
) -> Result<u64, StoreError> {
	let Vouched { records, end } = vouched::<L>(database, plane_id)
		.and_then(|vouched| {
			behind_store::<L>(vouched.records.len(), applied)?;
			Ok(vouched)
		})
		.map_err(|error| match error {
			StoreError::Integrity(detail) => untrusted::<L>(&detail),
			other => other,
		})?;
	let mut head = records.last().map_or(
		Head {
			sequence: 0,
			link: Link::genesis::<L>(plane_id),
		},
		|(record, link)| Head {
			sequence: L::sequence(record),
			link: *link,
		},
	);
	let mut body = String::new();
	for item in pending {
		let record = record(head.sequence + 1, item);
		head = Head {
			sequence: L::sequence(&record),
			link: Link::after::<L>(head.link, &record),
		};
		body.push_str(&format!(
			"{} {} {}\n",
			head.sequence,
			L::spell(&record),
			head.link
		));
	}
	files::write_lines::<L>(database, plane_id, end, &body)?;
	files::write_head::<L>(database, plane_id, head)?;
	Ok(head.sequence)
}

/// The records the head vouches for and where they end in the file.
fn vouched<L: Ledger>(
	database: &Path,
	plane_id: Uuid,
) -> Result<Vouched<L::Record>, StoreError> {
	let head = files::read_head::<L>(database, plane_id)?;
	let file = files::read_ledger_file::<L>(database, plane_id)?;
	match (head, file) {
		(None, None) => Ok(Vouched {
			records: vec![],
			end: None,
		}),
		(Some(head), None) => Err(StoreError::Integrity(format!(
			"its head names {} {}, but the ledger is missing",
			L::NOUN,
			head.sequence
		))),
		// A single line past a missing head is the first record, written
		// just before the crash that kept its head from following.
		(None, Some(file)) => match file.records.len() {
			0 | 1 => Ok(Vouched {
				records: vec![],
				end: Some(file.header_end),
			}),
			count => Err(StoreError::Integrity(format!(
				"it holds {count} {}, but its head is missing",
				L::PLURAL
			))),
		},
		(Some(head), Some(file)) => {
			let sequence = head.sequence;
			let vouched = usize::try_from(sequence).unwrap_or(usize::MAX);
			let count = file.records.len();
			if count < vouched {
				return Err(StoreError::Integrity(format!(
					"its head names {} {sequence}, but the ledger ends at \
					 {count}",
					L::NOUN
				)));
			}
			if count > vouched.saturating_add(1) {
				return Err(StoreError::Integrity(format!(
					"its head names {} {sequence}, but the ledger goes on \
					 to {count}",
					L::NOUN
				)));
			}
			// The head never names sequence zero; `read_head` refuses it.
			let newest = vouched.saturating_sub(1);
			if file.records[newest].1 != head.link {
				return Err(StoreError::Integrity(format!(
					"{} {sequence} is not the one its head names",
					L::NOUN
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
pub(crate) fn untrusted<L: Ledger>(detail: &str) -> StoreError {
	StoreError::Integrity(format!(
		"the {} cannot be trusted: {detail}",
		L::NAME
	))
}
