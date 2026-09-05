//! The durable head of the Security audit chain, kept outside the database
//! it describes (ADR-0105).
//!
//! Authoritative state is rollback-capable: a verified Recovery snapshot or
//! an imported bundle can put the database back the way it was an hour ago
//! (ADR-0097, ADR-0102). An audit whose only record of its own length lived
//! inside that database would go back with it and say nothing was missing.
//! The head therefore lives beside the store as its own small file, so a
//! store that has moved backwards is a store whose newest audit record the
//! head has never heard of.
//!
//! Recovery tooling that copies a Plane must copy this file deliberately:
//! taking it along with the database restores the pair consistently, and
//! leaving it behind is exactly the tamper signal it exists to raise.
//!
//! The file is not a secret and is not a defence against code already
//! running as the same operating-system user, which can rewrite both sides
//! (ADR-0105). It makes a rollback of one side visible from the other.

use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::StoreError;
use crate::audit_chain::AuditEntryHash;

/// Suffix appended to the database file name. Deriving it keeps two stores
/// in one directory from sharing a head.
const HEAD_SUFFIX: &str = ".audit-head";

/// Suffix of the file the next head is written to before it replaces the
/// current one.
const PENDING_SUFFIX: &str = ".audit-head.pending";

/// First line of the file, so a future format is recognized rather than
/// misread.
const FORMAT: &str = "jet-security-audit-head 1";

/// How far the Security audit chain had reached when it was last written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditHead {
	/// The epoch the newest record belongs to.
	pub epoch: u64,
	/// The sequence of the newest record.
	pub sequence: u64,
	/// The chain link that record folded to.
	pub entry_hash: AuditEntryHash,
}

/// Where the head of the audit for the store at `database` is kept.
#[must_use]
pub fn audit_head_path(database: &Path) -> PathBuf {
	sibling(database, HEAD_SUFFIX)
}

/// Reads the head beside `database`, or `None` when no audit has been
/// written on this Plane yet.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] when the file cannot be read and
/// [`StoreError::Integrity`] when it is not the head of `plane_id`.
pub(crate) fn read(
	database: &Path,
	plane_id: Uuid,
) -> Result<Option<AuditHead>, StoreError> {
	let path = audit_head_path(database);
	let text = match fs::read_to_string(&path) {
		Ok(text) => text,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			return Ok(None);
		}
		Err(error) => {
			return Err(StoreError::Unavailable(format!(
				"cannot read the Security audit head {}: {error}",
				path.display()
			)));
		}
	};
	parse(&text, plane_id).map(Some)
}

/// Replaces the head beside `database` with `head`.
///
/// The new head is written to its own file and renamed over the old one, so
/// a failure part-way leaves the previous head whole rather than a
/// truncated one that would read as tampering.
///
/// # Errors
///
/// Returns [`StoreError::Unavailable`] when the file cannot be written or
/// durably replaced.
pub(crate) fn write(
	database: &Path,
	plane_id: Uuid,
	head: AuditHead,
) -> Result<(), StoreError> {
	let path = audit_head_path(database);
	let pending = sibling(database, PENDING_SUFFIX);
	let AuditHead {
		epoch,
		sequence,
		entry_hash,
	} = head;
	let body = format!(
		"{FORMAT}\nplane {plane_id}\nepoch {epoch}\nsequence {sequence}\n\
		 hash {entry_hash}\n"
	);
	// ASVS 16.3.1: the head is owner-only, like everything under ~/.jet.
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
	fs::set_permissions(&pending, fs::Permissions::from_mode(0o600))
		.map_err(|error| unavailable(&pending, &error))?;
	drop(file);
	fs::rename(&pending, &path).map_err(|error| unavailable(&path, &error))?;
	// The rename itself has to reach the disk, or a power loss leaves the
	// previous head in place while the store has already committed past it.
	if let Some(directory) = path.parent() {
		File::open(directory)
			.and_then(|directory| directory.sync_all())
			.map_err(|error| unavailable(directory, &error))?;
	}
	Ok(())
}

fn parse(text: &str, plane_id: Uuid) -> Result<AuditHead, StoreError> {
	let mut lines = text.lines();
	let format = lines.next().unwrap_or_default();
	if format != FORMAT {
		return Err(StoreError::Integrity(format!(
			"the Security audit head is {format:?}, not {FORMAT:?}"
		)));
	}
	let recorded_plane = field(&mut lines, "plane")?;
	if recorded_plane != plane_id.to_string() {
		return Err(StoreError::Integrity(format!(
			"the Security audit head belongs to Plane {recorded_plane}, not \
			 {plane_id}"
		)));
	}
	Ok(AuditHead {
		epoch: number(&field(&mut lines, "epoch")?)?,
		sequence: number(&field(&mut lines, "sequence")?)?,
		entry_hash: AuditEntryHash::parse_hex(&field(&mut lines, "hash")?)?,
	})
}

fn field<'a>(
	lines: &mut impl Iterator<Item = &'a str>,
	name: &str,
) -> Result<String, StoreError> {
	let line = lines.next().unwrap_or_default();
	line.strip_prefix(name)
		.and_then(|rest| rest.strip_prefix(' '))
		.map(ToOwned::to_owned)
		.ok_or_else(|| {
			StoreError::Integrity(format!(
				"the Security audit head has {line:?} where its {name} \
				 belongs"
			))
		})
}

fn number(text: &str) -> Result<u64, StoreError> {
	text.parse().map_err(|error| {
		StoreError::Integrity(format!(
			"the Security audit head has {text:?} where a number belongs: \
			 {error}"
		))
	})
}

fn sibling(database: &Path, suffix: &str) -> PathBuf {
	let mut name = database.as_os_str().to_owned();
	name.push(suffix);
	PathBuf::from(name)
}

fn unavailable(path: &Path, error: &std::io::Error) -> StoreError {
	StoreError::Unavailable(format!(
		"cannot write the Security audit head {}: {error}",
		path.display()
	))
}
