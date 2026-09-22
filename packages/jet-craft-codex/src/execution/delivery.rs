//! Refuse replay when native input may have escaped an acknowledged checkpoint.
use jet_craft_sdk::CraftError;
use std::{
	io::{self, Write},
	os::unix::fs::OpenOptionsExt,
	path::{Path, PathBuf},
};

pub(super) struct Delivery {
	path: PathBuf,
	committed: u64,
	pending: bool,
}

impl Delivery {
	pub(super) async fn open(
		helper: &Path,
		committed: u64,
	) -> Result<Self, CraftError> {
		let path = helper.with_file_name("codex-input.pending");
		let check = path.clone();
		blocking(move || {
			match std::fs::symlink_metadata(&check) {
				Err(error) if error.kind() == io::ErrorKind::NotFound => {
					return Ok(());
				}
				Ok(metadata)
					if metadata.file_type().is_file()
						&& metadata.len() == 8 => {}
				_ => return Err(io::ErrorKind::InvalidData.into()),
			}
			let bytes: [u8; 8] = std::fs::read(&check)?
				.try_into()
				.map_err(|_| io::ErrorKind::InvalidData)?;
			if committed <= u64::from_le_bytes(bytes) {
				// The native process may already have acted on this input. Its
				// absence from the committed parser state is not proof of failure.
				return Err(io::ErrorKind::InvalidData.into());
			}
			// The host committed the subsequent checkpoint but its acknowledgement
			// was lost. That checkpoint already includes the changed parser state.
			remove(&check)
		})
		.await?;
		Ok(Self {
			path,
			committed,
			pending: false,
		})
	}

	pub(super) async fn before_input(&mut self) -> Result<(), CraftError> {
		if !self.pending {
			let path = self.path.clone();
			let committed = self.committed;
			blocking(move || {
				let mut file = std::fs::OpenOptions::new()
					.write(true)
					.create_new(true)
					.mode(0o600)
					.open(&path)?;
				file.write_all(&committed.to_le_bytes())?;
				file.sync_all()?;
				sync_parent(&path)
			})
			.await?;
			self.pending = true;
		}
		Ok(())
	}

	pub(super) async fn acknowledged(
		&mut self,
		source_offset: u64,
	) -> Result<(), CraftError> {
		if self.pending {
			let path = self.path.clone();
			blocking(move || remove(&path)).await?;
			self.pending = false;
		}
		self.committed = source_offset;
		Ok(())
	}
}

fn sync_parent(path: &Path) -> io::Result<()> {
	std::fs::File::open(path.parent().ok_or(io::ErrorKind::InvalidInput)?)?
		.sync_all()
}

fn remove(path: &Path) -> io::Result<()> {
	std::fs::remove_file(path)?;
	sync_parent(path)
}

async fn blocking(
	work: impl FnOnce() -> io::Result<()> + Send + 'static,
) -> Result<(), CraftError> {
	tokio::task::spawn_blocking(work)
		.await
		.map_err(|_| CraftError::Disconnected)?
		.map_err(|_| CraftError::InvalidMessage)
}
