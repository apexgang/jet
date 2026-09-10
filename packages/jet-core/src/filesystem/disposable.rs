//! Only the flat payload and cache namespaces participate in pressure cleanup.
use crate::{Core, CoreError, artifact::files};
use rustix::fs::{Mode, OFlags, open, openat};
use std::{collections::BTreeSet, fs::File, path::Path, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Namespace {
	Payloads,
	Cache,
}
pub(crate) const FOLDERS: [Namespace; 2] =
	[Namespace::Payloads, Namespace::Cache];
const BATCH_LIMIT: usize = 64;

pub(crate) struct Entry {
	pub(crate) directory: Arc<File>,
	pub(crate) file: File,
	pub(crate) name: String,
	pub(crate) active: bool,
	pub(crate) size: u64,
}

pub(crate) struct Batch {
	pub(crate) entries: Vec<Entry>,
	pub(crate) next: String,
}

pub(crate) fn batch(
	home: &Path,
	folder: Namespace,
	after: &str,
) -> Result<Batch, CoreError> {
	let flags =
		OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
	let directory = match folder {
		Namespace::Payloads => files::directory(home)?,
		Namespace::Cache => {
			let home =
				open(home, flags, Mode::empty()).map_err(files::io_error)?;
			match openat(&home, "cache", flags, Mode::empty()) {
				Ok(file) => File::from(file),
				Err(
					rustix::io::Errno::NOENT
					| rustix::io::Errno::NOTDIR
					| rustix::io::Errno::LOOP,
				) => {
					return Ok(Batch {
						entries: vec![],
						next: String::new(),
					});
				}
				Err(error) => return Err(files::io_error(error)),
			}
		}
	};
	let directory = Arc::new(directory);
	let mut names = BTreeSet::new();
	for entry in
		rustix::fs::Dir::read_from(&directory).map_err(files::io_error)?
	{
		let entry = entry.map_err(files::io_error)?;
		let Ok(name) = entry.file_name().to_str() else {
			continue;
		};
		// ASVS 5.3.2: never traverse Workspaces, recovery, or arbitrary cache paths.
		if name > after
			&& (crate::artifact::validate_hash(name).is_ok()
				|| name
					.strip_prefix(".pending-")
					.is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()))
		{
			names.insert(name.to_owned());
			if names.len() > BATCH_LIMIT {
				names.pop_last();
			}
		}
	}
	let next = if names.len() == BATCH_LIMIT {
		names.last().expect("full batch").clone()
	} else {
		String::new()
	};
	let mut entries = Vec::new();
	for name in names {
		let Ok(file) = files::open(&directory, &name) else {
			continue;
		};
		let active = file.try_lock().is_err();
		let size = file.metadata().map_err(files::io_error)?.len();
		entries.push(Entry {
			size,
			directory: Arc::clone(&directory),
			file,
			name,
			active,
		});
	}
	Ok(Batch { entries, next })
}

impl Core {
	pub(crate) async fn disposable_budget(&self) -> Result<u64, CoreError> {
		self.store
			.read(async |tx| {
				let stored = tx
					.settings_for_scope(crate::SettingScope::Plane.record())
					.await?;
				let values = crate::setting::resolve(
					&[crate::SettingKey::StorageDisposableMiB],
					&stored,
				);
				let crate::SettingValue::Count(mib) = values[0].value else {
					return Err(files::corrupt());
				};
				Ok(u64::from(mib) * 1024 * 1024)
			})
			.await
	}

	/// Called under the publication gate. References protect bytes regardless of
	/// Conversation pins, lifecycle, or the cleanliness of its Workspace.
	pub(crate) async fn check_disposable(
		&self,
		incoming: u64,
	) -> Result<(), CoreError> {
		if incoming > self.disposable_remaining().await? {
			return Err(crate::disk_pressure::pressure());
		}
		Ok(())
	}

	pub(crate) async fn disposable_remaining(&self) -> Result<u64, CoreError> {
		let budget = self.disposable_budget().await?;
		let mut used = 0u64;
		for folder in FOLDERS {
			let mut after = String::new();
			loop {
				if used > budget {
					return Err(crate::disk_pressure::pressure());
				}
				let home = self.run_home();
				let page = crate::filesystem::blocking(move || {
					batch(&home, folder, &after)
				})
				.await??;
				for entry in page.entries {
					if folder == Namespace::Payloads
						&& self
							.store
							.read(async |tx| {
								tx.artifact_referenced(&entry.name).await
							})
							.await?
					{
						continue;
					}
					used = used.saturating_add(entry.size);
				}
				if page.next.is_empty() {
					break;
				}
				after = page.next;
			}
		}
		budget
			.checked_sub(used)
			.ok_or_else(crate::disk_pressure::pressure)
	}
}
