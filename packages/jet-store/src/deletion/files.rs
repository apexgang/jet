//! The two files behind the ledger: their names, their lines, and the
//! durable writes that keep them consistent (ADR-0102).

use crate::{
	StoreError,
	deletion::ledger::{DeletedIdentityKind, DeletionRecord},
};
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
pub(super) const LEDGER_SUFFIX: &str = ".deletions";
pub(super) const HEAD_SUFFIX: &str = ".deletions.head";
const PENDING_HEAD_SUFFIX: &str = ".deletions.head.pending";

/// First lines of the files, so a future format is recognized rather than
/// misread.
const LEDGER_FORMAT: &str = "jet-deletion-ledger 1";
const HEAD_FORMAT: &str = "jet-deletion-ledger-head 1";

const GENESIS_DOMAIN: &[u8] = b"jet-deletion-ledger-genesis-v1";
const RECORD_DOMAIN: &[u8] = b"jet-deletion-ledger-record-v1";

/// What the head names: the newest sequence and its link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Head {
	pub(super) sequence: u64,
	pub(super) link: Link,
}

/// One link of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Link([u8; 32]);

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
	pub(super) fn genesis(plane_id: Uuid) -> Self {
		let mut hasher = Sha256::new();
		hasher.update(GENESIS_DOMAIN);
		hasher.update(plane_id.as_bytes());
		Self(hasher.finalize().into())
	}

	/// The link that follows `previous` for `record`.
	pub(super) fn after(previous: Self, record: &DeletionRecord) -> Self {
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

/// The file's view of the ledger, before the head has been consulted.
pub(super) struct LedgerFile {
	/// Every well-formed record in file order, with the link it folds to.
	pub(super) records: Vec<(DeletionRecord, Link)>,
	/// Where each record's line ends, so an unconfirmed tail can be cut.
	pub(super) ends: Vec<u64>,
	/// Where the header ends.
	pub(super) header_end: u64,
}

/// Where the Recovery evidence of the store at `database` lives.
#[must_use]
pub(crate) fn recovery_dir(database: &Path) -> PathBuf {
	database.parent().map_or_else(
		|| PathBuf::from(DIRECTORY),
		|parent| parent.join(DIRECTORY),
	)
}

pub(super) fn evidence_path(database: &Path, suffix: &str) -> PathBuf {
	let mut name = database.file_name().unwrap_or_default().to_owned();
	name.push(suffix);
	recovery_dir(database).join(name)
}

/// Appends `lines` to the ledger file durably, after the header when
/// the file is new and after `end` otherwise, so that whatever lay past
/// the vouched-for part is written over.
pub(super) fn write_lines(
	database: &Path,
	plane_id: Uuid,
	end: Option<u64>,
	lines: &str,
) -> Result<(), StoreError> {
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
	body.push_str(lines);
	file.seek(SeekFrom::End(0))
		.and_then(|_| file.write_all(body.as_bytes()))
		.and_then(|()| file.sync_all())
		.map_err(|error| unavailable(&path, &error))
}

/// Reads the ledger file and folds its chain, or `None` when there is
/// none. Every line must fold to the link it carries; the head decides
/// how many of them count.
pub(super) fn read_ledger_file(
	database: &Path,
	plane_id: Uuid,
) -> Result<Option<LedgerFile>, StoreError> {
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
	let plane = check_plane(plane_line, plane_id)?;
	let mut link = Link::genesis(plane);
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
	Ok(Some(LedgerFile {
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

pub(super) fn read_head(
	database: &Path,
	plane_id: Uuid,
) -> Result<Option<Head>, StoreError> {
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
	Ok(Some(Head { sequence, link }))
}

pub(super) fn write_head(
	database: &Path,
	plane_id: Uuid,
	head: Head,
) -> Result<(), StoreError> {
	let path = evidence_path(database, HEAD_SUFFIX);
	let pending = evidence_path(database, PENDING_HEAD_SUFFIX);
	let Head { sequence, link } = head;
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

/// The Plane a file names, checked against `plane_id` when that is known.
/// A damaged store whose Plane row cannot be read has a nil identity, and
/// the file's own line is then taken as read, so the chain still folds
/// from the Plane it was written for.
fn check_plane(line: &str, plane_id: Uuid) -> Result<Uuid, StoreError> {
	let recorded = line
		.strip_prefix("plane ")
		.and_then(|text| Uuid::parse_str(text).ok())
		.ok_or_else(|| {
			StoreError::Integrity("it does not name its Plane".into())
		})?;
	if !plane_id.is_nil() && recorded != plane_id {
		return Err(StoreError::Integrity(format!(
			"it belongs to Plane {recorded}, not {plane_id}"
		)));
	}
	Ok(recorded)
}

fn unavailable(path: &Path, error: &std::io::Error) -> StoreError {
	StoreError::Unavailable(format!(
		"cannot use the Deletion ledger {}: {error}",
		path.display()
	))
}
