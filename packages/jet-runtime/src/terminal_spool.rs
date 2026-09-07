//! Bounded raw PTY replay, including a clean shell's final output.
use serde::{Deserialize, Serialize};
use std::{
	fs::{File, OpenOptions},
	io,
	os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
	path::{Path, PathBuf},
};

const LIMIT: u64 = 8 * 1024 * 1024;
#[derive(Default, Serialize, Deserialize)]
struct State {
	produced: u64,
	closed: bool,
}

/// A retained raw byte range. A larger offset than requested declares a gap.
pub struct TerminalReplay {
	/// First returned byte.
	pub offset: u64,
	/// End of known output.
	pub produced: u64,
	/// Bounded raw bytes.
	pub bytes: Vec<u8>,
	/// Output ended.
	pub closed: bool,
}

/// An owner-only eight-MiB file ring, owned exclusively by one helper.
pub struct TerminalSpool {
	file: File,
	directory: PathBuf,
	state: State,
	failed: bool,
}
impl TerminalSpool {
	/// Creates a new bounded ring. Returns filesystem errors or an existing-file refusal.
	pub fn create(directory: &Path) -> io::Result<Self> {
		crate::validate_execution_directory(directory)?;
		let file = OpenOptions::new()
			.read(true)
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(directory.join("output.bin"))?;
		file.set_len(LIMIT)?;
		let spool = Self {
			file,
			directory: directory.into(),
			state: State::default(),
			failed: false,
		};
		spool.publish()?;
		Ok(spool)
	}
	/// Appends one bounded chunk, displacing the oldest raw bytes.
	/// Returns IO or offset-overflow errors; the helper must stop on retention failure.
	pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
		if bytes.len() > 65536 || self.state.closed || self.failed {
			return Err(invalid());
		}
		let end = self
			.state
			.produced
			.checked_add(bytes.len() as u64)
			.ok_or_else(invalid)?;
		let start = self.state.produced % LIMIT;
		let split = bytes.len().min((LIMIT - start) as usize);
		self.state.produced = end;
		let result = self
			.file
			.write_all_at(&bytes[..split], start)
			.and_then(|()| self.file.write_all_at(&bytes[split..], 0))
			.and_then(|()| self.publish());
		self.failed = result.is_err();
		result
	}
	/// Records clean EOF after the PTY process is reaped. Returns filesystem errors.
	pub fn finish(&mut self) -> io::Result<()> {
		if self.failed {
			return Err(invalid());
		}
		self.state.closed = true;
		self.publish()
	}
	/// Reads within a caller's byte credit. Returns invalid-range or IO errors.
	pub fn read(&self, after: u64, limit: u32) -> io::Result<TerminalReplay> {
		if after > self.state.produced || limit > 65536 {
			return Err(invalid());
		}
		if self.failed {
			return Ok(TerminalReplay {
				offset: self.state.produced,
				produced: self.state.produced,
				bytes: vec![],
				closed: false,
			});
		}
		let offset = after.max(self.state.produced.saturating_sub(LIMIT));
		let length =
			(self.state.produced - offset).min(u64::from(limit)) as usize;
		let mut bytes = vec![0; length];
		let start = offset % LIMIT;
		let split = length.min((LIMIT - start) as usize);
		self.file.read_exact_at(&mut bytes[..split], start)?;
		self.file.read_exact_at(&mut bytes[split..], 0)?;
		Ok(TerminalReplay {
			offset,
			produced: self.state.produced,
			bytes,
			closed: self.state.closed,
		})
	}
	/// Reads after a helper is proven gone. Incomplete writes become an explicit
	/// gap; only a cleanly finished ring is eligible for replay.
	/// Returns unsafe-file, range, and IO errors.
	pub fn read_retained(
		directory: &Path,
		after: u64,
		limit: u32,
	) -> io::Result<TerminalReplay> {
		let bytes = crate::read_execution_file(&directory.join("replay.json"))?;
		let state: State =
			serde_json::from_slice(&bytes).map_err(io::Error::other)?;
		if !state.closed {
			let end = after.max(state.produced);
			return Ok(TerminalReplay {
				offset: end,
				produced: end,
				bytes: vec![],
				closed: true,
			});
		}
		// ASVS 5.3.2: inspect the opened object and never follow a spool symlink.
		let file = OpenOptions::new()
			.read(true)
			.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
			.open(directory.join("output.bin"))?;
		let metadata = file.metadata()?;
		if !metadata.is_file()
			|| metadata.len() != LIMIT
			|| metadata.uid() != rustix::process::geteuid().as_raw()
			|| metadata.mode() & 0o077 != 0
		{
			return Err(invalid());
		}
		Self {
			file,
			directory: directory.into(),
			state,
			failed: false,
		}
		.read(after, limit)
	}
	fn publish(&self) -> io::Result<()> {
		let temporary = self.directory.join("replay.pending");
		let mut file = OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(&temporary)?;
		serde_json::to_writer(&mut file, &self.state)
			.map_err(io::Error::other)?;
		// The helper survives jetd failure within one OS boot. No per-chunk fsync.
		std::fs::rename(temporary, self.directory.join("replay.json"))
	}
}
fn invalid() -> io::Error {
	io::Error::other("invalid terminal replay state")
}
