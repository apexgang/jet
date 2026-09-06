//! Bounded disk replay. Native source is released only after durable acknowledgement.
use jet_protocol::{HelperEvent, HelperRecord, decode_control, encode_control};
use std::{
	collections::VecDeque,
	fs,
	path::PathBuf,
	sync::{Arc, Mutex},
};
use tokio::sync::Notify;

const LIMIT: u64 = 64 * 1024 * 1024;
#[derive(Default)]
struct State {
	offset: u64,
	bytes: u64,
	entries: VecDeque<(u64, u64)>,
	ended: bool,
}
pub(crate) struct Spool {
	directory: PathBuf,
	state: Mutex<State>,
	changed: Notify,
}
impl Spool {
	pub(crate) fn new(directory: PathBuf) -> Arc<Self> {
		Arc::new(Self {
			directory,
			state: Mutex::new(State::default()),
			changed: Notify::new(),
		})
	}
	pub(crate) async fn append(
		&self,
		event: HelperEvent,
	) -> std::io::Result<()> {
		loop {
			let changed = self.changed.notified();
			if self.try_append(&event)? {
				self.changed.notify_waiters();
				return Ok(());
			}
			changed.await;
		}
	}
	fn try_append(&self, event: &HelperEvent) -> std::io::Result<bool> {
		let mut state = self.state.lock().expect("spool lock poisoned");
		let mut record = HelperRecord {
			source_offset: state.offset,
			event: event.clone(),
		};
		// Reserve the maximum decimal offset width when calculating record space.
		let size = encode_control(&record)
			.map_err(std::io::Error::other)?
			.len() as u64
			+ 20;
		if state.bytes + size > LIMIT {
			return Ok(false);
		}
		record.source_offset = state.offset + size;
		let bytes = encode_control(&record).map_err(std::io::Error::other)?;
		let path = self
			.directory
			.join(format!("{}.json", record.source_offset));
		// The directory is owner-only; sync before making a source record visible.
		let mut file = fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(path)?;
		std::io::Write::write_all(&mut file, &bytes)?;
		file.sync_all()?;
		state.offset = record.source_offset;
		state.bytes += size;
		state.entries.push_back((record.source_offset, size));
		state.ended = matches!(
			event,
			HelperEvent::Exited { .. } | HelperEvent::LaunchFailed
		);
		Ok(true)
	}
	pub(crate) async fn next(&self) -> std::io::Result<Option<HelperRecord>> {
		loop {
			let changed = self.changed.notified();
			{
				let state = self.state.lock().expect("spool lock poisoned");
				if let Some((offset, _)) = state.entries.front() {
					let bytes = fs::read(
						self.directory.join(format!("{offset}.json")),
					)?;
					return decode_control(&bytes)
						.map(Some)
						.map_err(std::io::Error::other);
				}
				if state.ended {
					return Ok(None);
				}
			}
			changed.await;
		}
	}
	pub(crate) async fn acknowledge(&self, offset: u64) -> std::io::Result<()> {
		let mut state = self.state.lock().expect("spool lock poisoned");
		let Some(&(expected, size)) = state.entries.front() else {
			return Err(std::io::Error::other("no pending source"));
		};
		// ASVS 2.3.1: a future or out-of-order acknowledgement releases nothing.
		if offset != expected {
			return Err(std::io::Error::other(
				"invalid source acknowledgement",
			));
		}
		fs::remove_file(self.directory.join(format!("{offset}.json")))?;
		state.entries.pop_front();
		state.bytes -= size;
		drop(state);
		self.changed.notify_waiters();
		Ok(())
	}
}
