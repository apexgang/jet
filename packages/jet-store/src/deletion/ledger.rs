//! The ledger file and its head: two owner-only text files under the
//! store's `recovery` directory, read whole and appended to durably
//! (ADR-0102).
//!
//! The ledger holds one line per deletion, each folding the line before it
//! into a link. The head names the newest link the ledger vouches for and
//! is replaced atomically after the ledger has reached the disk, so a
//! crash between the two leaves at most one line past the head, which the
//! next append discards. Anything else that does not fold through the head
//! is corruption, and the ledger then vouches for nothing.

use crate::{StoreError, deletion::PendingDeletion};
use sha2::{Digest as _, Sha256};
use std::{
	fmt,
	fs::{self, File},
	io::{Seek as _, SeekFrom, Write as _},
	os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
	path::{Path, PathBuf},
};
use uuid::Uuid;

/// Directory beside the database that holds Recovery evidence outside it.
const DIRECTORY: &str = "recovery";

/// Suffixes appended to the database file name. Deriving them keeps two
/// stores in one directory from sharing a ledger.
const LEDGER_SUFFIX: &str = ".deletions";
const HEAD_SUFFIX: &str = ".deletions.head";
const PENDING_HEAD_SUFFIX: &str = ".deletions.head.pending";

/// First lines of the files, so a future format is recognized rather than
/// misread.
const LEDGER_FORMAT: &str = "jet-deletion-ledger 1";
const HEAD_FORMAT: &str = "jet-deletion-ledger-head 1";

const GENESIS_DOMAIN: &[u8] = b"jet-deletion-ledger-genesis-v1";
const RECORD_DOMAIN: &[u8] = b"jet-deletion-ledger-record-v1";

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

	fn parse(text: &str) -> Option<Self> {
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
	/// When the newest recorded deletion was acknowledged, or `None` when
	/// there is none the ledger vouches for.
	#[must_use]
	pub fn newest_deletion_unix_ms(&self) -> Option<i64> {
		match self {
			Self::Verified(records) => {
				records.iter().map(|record| record.deleted_at_unix_ms).max()
			}
			Self::Corrupt(_) => None,
		}
	}
}

/// One link of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link([u8; 32]);

impl fmt::Display for Link {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		for byte in self.0 {
			write!(formatter, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl Link {
	fn parse(text: &str) -> Option<Self> {
		if text.len() != 64 {
			return None;
		}
		let mut bytes = [0_u8; 32];
		for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
			*byte =
				u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
		}
		Some(Self(bytes))
	}

	/// The link the first record follows. It binds the chain to its Plane.
	fn genesis(plane_id: Uuid) -> Self {
		let mut hasher = Sha256::new();
		hasher.update(GENESIS_DOMAIN);
		hasher.update(plane_id.as_bytes());
		Self(hasher.finalize().into())
	}

	/// The link that follows `previous` for `record`.
	fn after(previous: Self, record: &DeletionRecord) -> Self {
		let mut hasher = Sha256::new();
		hasher.update(RECORD_DOMAIN);
		hasher.update(previous.0);
		hasher.update(record.sequence.to_be_bytes());
		hasher.update(record.deleted_at_unix_ms.to_be_bytes());
		let kind = record.kind.as_str().as_bytes();
		hasher.update(
			u64::try_from(kind.len()).unwrap_or(u64::MAX).to_be_bytes(),
		);
		hasher.update(kind);
		hasher.update(record.identity.as_bytes());
		Self(hasher.finalize().into())
	}
}

/// What the head vouches for.
struct Confirmed {
	/// The records the head reaches, with their links.
	records: Vec<(DeletionRecord, Link)>,
	/// Where the confirmed part of the ledger ends in the file, or `None`
	/// when there is no file yet.
	end: Option<u64>,
}

/// The ledger as read from disk, before the head has been consulted.
struct Loaded {
	/// Every well-formed record in file order, with the link it folds to.
	records: Vec<(DeletionRecord, Link)>,
	/// Where each record's line ends, so an unconfirmed tail can be cut.
	ends: Vec<u64>,
	/// Where the header ends.
	header_end: u64,
}

/// Where the Recovery evidence of the store at `database` lives.
#[must_use]
pub(crate) fn recovery_dir(database: &Path) -> PathBuf {
	database.parent().map_or_else(
		|| PathBuf::from(DIRECTORY),
		|parent| parent.join(DIRECTORY),
	)
}

fn evidence_path(database: &Path, suffix: &str) -> PathBuf {
	let mut name = database.file_name().unwrap_or_default().to_owned();
	name.push(suffix);
	recovery_dir(database).join(name)
}

/// Reads and verifies the ledger of the store at `database`.
///
/// `plane_id` is the Plane the ledger must belong to. A damaged store
/// whose Plane row cannot be read has a nil identity, and the ledger's own
/// Plane line is then taken as read.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] when a file cannot be read. A file
/// that reads but does not verify is reported as
/// [`DeletionLedger::Corrupt`], not as an error.
pub(crate) fn read(
	database: &Path,
	plane_id: Uuid,
) -> Result<DeletionLedger, StoreError> {
	match verified(database, plane_id) {
		Ok(confirmed) => Ok(DeletionLedger::Verified(
			confirmed
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

/// Appends `deletions` to the ledger durably and advances its head.
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
	deletions: &[PendingDeletion],
) -> Result<(), StoreError> {
	let Confirmed { records, end } =
		verified(database, plane_id).map_err(|error| match error {
			StoreError::Integrity(detail) => StoreError::Integrity(format!(
				"the Deletion ledger cannot be trusted: {detail}"
			)),
			other => other,
		})?;
	let path = evidence_path(database, LEDGER_SUFFIX);
	let directory = recovery_dir(database);
	// ASVS 16.3.1: the evidence is owner-only, like everything under ~/.jet.
	fs::DirBuilder::new()
		.recursive(true)
		.mode(0o700)
		.create(&directory)
		.map_err(|error| unavailable(&directory, &error))?;
	let mut file = fs::OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(false)
		.mode(0o600)
		.open(&path)
		.map_err(|error| unavailable(&path, &error))?;
	let (mut sequence, mut link) = records
		.last()
		.map_or((0, Link::genesis(plane_id)), |(record, link)| {
			(record.sequence, *link)
		});
	let mut body = String::new();
	match end {
		// The file is new: it starts with its header.
		None => body.push_str(&format!("{LEDGER_FORMAT}\nplane {plane_id}\n")),
		// A line past the head is a deletion no store commit followed;
		// the ledger vouches for nothing past the head, so it goes.
		Some(end) => file
			.set_len(end)
			.map_err(|error| unavailable(&path, &error))?,
	}
	for deletion in deletions {
		sequence += 1;
		let record = DeletionRecord {
			sequence,
			deleted_at_unix_ms: deletion.deleted_at_unix_ms,
			kind: deletion.kind,
			identity: deletion.identity,
		};
		link = Link::after(link, &record);
		body.push_str(&format!(
			"{sequence} {} {} {} {link}\n",
			record.deleted_at_unix_ms,
			record.kind.as_str(),
			record.identity
		));
	}
	file.seek(SeekFrom::End(0))
		.and_then(|_| file.write_all(body.as_bytes()))
		.and_then(|()| file.sync_all())
		.map_err(|error| unavailable(&path, &error))?;
	drop(file);
	write_head(database, plane_id, sequence, link)
}

/// The records the head vouches for and where they end in the file.
fn verified(database: &Path, plane_id: Uuid) -> Result<Confirmed, StoreError> {
	let head = read_head(database, plane_id)?;
	let loaded = load(database, plane_id)?;
	match (head, loaded) {
		(None, None) => Ok(Confirmed {
			records: vec![],
			end: None,
		}),
		(Some((sequence, _)), None) => Err(StoreError::Integrity(format!(
			"its head names deletion {sequence}, but the ledger is missing"
		))),
		// A single line past a missing head is the first deletion, written
		// just before the crash that kept its head from following.
		(None, Some(loaded)) => match loaded.records.len() {
			0 | 1 => Ok(Confirmed {
				records: vec![],
				end: Some(loaded.header_end),
			}),
			count => Err(StoreError::Integrity(format!(
				"it holds {count} deletions, but its head is missing"
			))),
		},
		(Some((sequence, link)), Some(loaded)) => {
			let confirmed = usize::try_from(sequence).unwrap_or(usize::MAX);
			let count = loaded.records.len();
			if count < confirmed {
				return Err(StoreError::Integrity(format!(
					"its head names deletion {sequence}, but the ledger ends \
					 at {count}"
				)));
			}
			if count > confirmed.saturating_add(1) {
				return Err(StoreError::Integrity(format!(
					"its head names deletion {sequence}, but the ledger goes \
					 on to {count}"
				)));
			}
			let Some((_, newest)) = loaded.records.get(confirmed - 1) else {
				return Err(StoreError::Integrity(
					"its head names deletion 0".into(),
				));
			};
			if *newest != link {
				return Err(StoreError::Integrity(format!(
					"deletion {sequence} is not the one its head names"
				)));
			}
			let mut records = loaded.records;
			records.truncate(confirmed);
			Ok(Confirmed {
				end: Some(loaded.ends[confirmed - 1]),
				records,
			})
		}
	}
}

/// Reads the ledger file and folds its chain, or `None` when there is
/// none. Every line must fold to the link it carries; the head decides
/// how many of them count.
fn load(database: &Path, plane_id: Uuid) -> Result<Option<Loaded>, StoreError> {
	let path = evidence_path(database, LEDGER_SUFFIX);
	let text = match fs::read_to_string(&path) {
		Ok(text) => text,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Ok(None);
		}
		Err(error) => return Err(unavailable(&path, &error)),
	};
	let mut lines = text.split_inclusive('\n');
	let mut offset = 0_u64;
	let mut take = || {
		lines.next().map(|line| {
			offset += u64::try_from(line.len()).unwrap_or(u64::MAX);
			(line.trim_end_matches('\n'), offset)
		})
	};
	let (format, _) = take().unwrap_or_default();
	if format != LEDGER_FORMAT {
		return Err(StoreError::Integrity(format!(
			"it is {format:?}, not {LEDGER_FORMAT:?}"
		)));
	}
	let (plane_line, header_end) = take().unwrap_or_default();
	check_plane(plane_line, plane_id)?;
	let mut link = Link::genesis(plane_id);
	let mut records = vec![];
	let mut ends = vec![];
	while let Some((line, end)) = take() {
		let sequence = u64::try_from(records.len()).unwrap_or(u64::MAX) + 1;
		let Some((record, recorded)) = parse_record(line) else {
			return Err(StoreError::Integrity(format!(
				"deletion {sequence} is malformed"
			)));
		};
		link = Link::after(link, &record);
		if record.sequence != sequence || recorded != link {
			return Err(StoreError::Integrity(format!(
				"deletion {sequence} was altered"
			)));
		}
		records.push((record, link));
		ends.push(end);
	}
	Ok(Some(Loaded {
		records,
		ends,
		header_end,
	}))
}

fn parse_record(line: &str) -> Option<(DeletionRecord, Link)> {
	let mut fields = line.split(' ');
	let record = DeletionRecord {
		sequence: fields.next()?.parse().ok()?,
		deleted_at_unix_ms: fields.next()?.parse().ok()?,
		kind: DeletedIdentityKind::parse(fields.next()?)?,
		identity: Uuid::parse_str(fields.next()?).ok()?,
	};
	let link = Link::parse(fields.next()?)?;
	fields.next().is_none().then_some((record, link))
}

fn read_head(
	database: &Path,
	plane_id: Uuid,
) -> Result<Option<(u64, Link)>, StoreError> {
	let path = evidence_path(database, HEAD_SUFFIX);
	let text = match fs::read_to_string(&path) {
		Ok(text) => text,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Ok(None);
		}
		Err(error) => return Err(unavailable(&path, &error)),
	};
	let mut lines = text.lines();
	let format = lines.next().unwrap_or_default();
	if format != HEAD_FORMAT {
		return Err(StoreError::Integrity(format!(
			"its head is {format:?}, not {HEAD_FORMAT:?}"
		)));
	}
	check_plane(lines.next().unwrap_or_default(), plane_id)?;
	let malformed = || StoreError::Integrity("its head is malformed".into());
	let sequence = lines
		.next()
		.and_then(|line| line.strip_prefix("sequence "))
		.and_then(|text| text.parse::<u64>().ok())
		.ok_or_else(malformed)?;
	let link = lines
		.next()
		.and_then(|line| line.strip_prefix("link "))
		.and_then(Link::parse)
		.ok_or_else(malformed)?;
	if sequence == 0 {
		return Err(malformed());
	}
	Ok(Some((sequence, link)))
}

fn write_head(
	database: &Path,
	plane_id: Uuid,
	sequence: u64,
	link: Link,
) -> Result<(), StoreError> {
	let path = evidence_path(database, HEAD_SUFFIX);
	let pending = evidence_path(database, PENDING_HEAD_SUFFIX);
	let body = format!(
		"{HEAD_FORMAT}\nplane {plane_id}\nsequence {sequence}\nlink {link}\n"
	);
	let mut file = fs::OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(true)
		.mode(0o600)
		.open(&pending)
		.map_err(|error| unavailable(&pending, &error))?;
	file.write_all(body.as_bytes())
		.and_then(|()| file.sync_all())
		.map_err(|error| unavailable(&pending, &error))?;
	drop(file);
	fs::rename(&pending, &path).map_err(|error| unavailable(&path, &error))?;
	// The rename itself has to reach the disk, or a power loss leaves the
	// previous head in place while the store has already committed past it.
	let directory = recovery_dir(database);
	File::open(&directory)
		.and_then(|directory| directory.sync_all())
		.map_err(|error| unavailable(&directory, &error))
}

fn check_plane(line: &str, plane_id: Uuid) -> Result<(), StoreError> {
	let Some(recorded) = line.strip_prefix("plane ") else {
		return Err(StoreError::Integrity("it does not name its Plane".into()));
	};
	if plane_id != Uuid::nil() && recorded != plane_id.to_string() {
		return Err(StoreError::Integrity(format!(
			"it belongs to Plane {recorded}, not {plane_id}"
		)));
	}
	Ok(())
}

fn unavailable(path: &Path, error: &std::io::Error) -> StoreError {
	StoreError::Unavailable(format!(
		"cannot use the Deletion ledger {}: {error}",
		path.display()
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

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
	/// back gives every record in order.
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

		append(&database, plane_id, &[first, second]).unwrap();
		append(&database, plane_id, &[third]).unwrap();

		let mode = |suffix| {
			fs::metadata(evidence_path(&database, suffix))
				.unwrap()
				.permissions()
				.mode() & 0o777
		};
		assert_eq!(
			(
				read(&database, plane_id).unwrap(),
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
		let empty = read(&database, plane_id).unwrap();
		append(
			&database,
			plane_id,
			&[
				deletion(DeletedIdentityKind::PairedClient, NOW_UNIX_MS),
				deletion(DeletedIdentityKind::Schedule, NOW_UNIX_MS + 1),
			],
		)
		.unwrap();
		let ledger = evidence_path(&database, LEDGER_SUFFIX);
		let original = fs::read_to_string(&ledger).unwrap();
		let other = Uuid::now_v7();
		let other_plane = read(&database, other).unwrap();

		let mut altered: Vec<String> =
			original.lines().map(ToOwned::to_owned).collect();
		altered[2] = altered[2].replacen("paired_client", "schedule", 1);
		fs::write(&ledger, altered.join("\n") + "\n").unwrap();
		let edited = read(&database, plane_id).unwrap();

		let shortened: Vec<&str> = original.lines().take(3).collect();
		fs::write(&ledger, shortened.join("\n") + "\n").unwrap();
		let cut = read(&database, plane_id).unwrap();

		fs::remove_file(&ledger).unwrap();
		let headless_ledger = read(&database, plane_id).unwrap();

		fs::write(&ledger, &original).unwrap();
		fs::remove_file(evidence_path(&database, HEAD_SUFFIX)).unwrap();
		let ledger_without_head = read(&database, plane_id).unwrap();

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
		append(&database, plane_id, &[first]).unwrap();
		let head = fs::read(evidence_path(&database, HEAD_SUFFIX)).unwrap();
		let lost = deletion(DeletedIdentityKind::Schedule, NOW_UNIX_MS + 1);
		append(&database, plane_id, &[lost]).unwrap();
		fs::write(evidence_path(&database, HEAD_SUFFIX), head).unwrap();
		let unconfirmed = read(&database, plane_id).unwrap();

		let next =
			deletion(DeletedIdentityKind::AccountBinding, NOW_UNIX_MS + 2);
		append(&database, plane_id, &[next]).unwrap();

		assert_eq!(
			(unconfirmed, read(&database, plane_id).unwrap()),
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
