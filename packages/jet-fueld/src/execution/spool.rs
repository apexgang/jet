//! Bounded disk replay. Native source is released only after durable acknowledgement.
use jet_protocol::{HelperEvent, HelperRecord, decode_control, encode_control};
use std::{
	collections::VecDeque,
	fs,
	os::unix::fs::OpenOptionsExt,
	path::PathBuf,
	sync::{Arc, Mutex},
};
use tokio::sync::Notify;

const LIMIT: u64 = 64 * 1024 * 1024;
#[derive(Default)]
struct State {
	offset: u64,
	acknowledged: u64,
	bytes: u64,
	entries: VecDeque<(u64, u64)>,
	ended: bool,
}
pub(crate) struct Spool {
	directory: PathBuf,
	identity: jet_protocol::HelperDescriptor,
	state: Mutex<State>,
	changed: Notify,
}
impl Spool {
	pub(crate) fn new(
		directory: PathBuf,
		identity: jet_protocol::HelperDescriptor,
	) -> Arc<Self> {
		Arc::new(Self {
			directory,
			identity,
			state: Mutex::new(State::default()),
			changed: Notify::new(),
		})
	}
	pub(crate) fn descriptor(&self) -> jet_protocol::HelperDescriptor {
		self.descriptor_at(&self.state.lock().expect("spool lock poisoned"))
	}
	fn descriptor_at(&self, state: &State) -> jet_protocol::HelperDescriptor {
		let mut descriptor = self.identity.clone();
		descriptor.replay = jet_protocol::HelperReplay {
			acknowledged: state.acknowledged,
			produced: state.offset,
			next_offset: state.entries.front().map(|(offset, _)| *offset),
		};
		descriptor
	}
	fn publish(&self, state: &State) -> std::io::Result<()> {
		let bytes = encode_control(&self.descriptor_at(state))
			.map_err(std::io::Error::other)?;
		let pending = self.directory.join("descriptor.pending");
		let mut file = fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(&pending)?;
		std::io::Write::write_all(&mut file, &bytes)?;
		fs::rename(pending, self.directory.join("descriptor.json"))
	}

	pub(crate) fn bounds(&self) -> (u64, u64) {
		let state = self.state.lock().expect("spool lock poisoned");
		(state.acknowledged, state.offset)
	}
	pub(crate) fn validate_offset(&self, offset: u64) -> std::io::Result<()> {
		let state = self.state.lock().expect("spool lock poisoned");
		if offset != state.acknowledged
			&& Some(offset) != state.entries.front().map(|(end, _)| *end)
		{
			return Err(std::io::Error::other(
				"recovery source boundary is unavailable",
			));
		}
		Ok(())
	}
	pub(crate) async fn recover(&self, offset: u64) -> std::io::Result<()> {
		self.validate_offset(offset)?;
		while self.bounds().0 < offset {
			let next = self
				.next()
				.await?
				.ok_or_else(|| std::io::Error::other("source ended"))?;
			self.acknowledge(next.source_offset).await?;
		}
		Ok(())
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
		// The helper survives daemon/Craft failure in the same OS. Completed
		// file writes retain source; per-record power-loss fsync would throttle
		// native output. An OS reboot loses the execution and is never replayed.
		let mut file = fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(path)?;
		std::io::Write::write_all(&mut file, &bytes)?;
		state.offset = record.source_offset;
		state.bytes += size;
		state.entries.push_back((record.source_offset, size));
		state.ended = matches!(
			event,
			HelperEvent::Exited { .. } | HelperEvent::LaunchFailed
		);
		self.publish(&state)?;
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
		state.acknowledged = offset;
		self.publish(&state)?;
		drop(state);
		self.changed.notify_waiters();
		Ok(())
	}
}
