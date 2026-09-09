//! Grace-period collection of payloads with no committed Run/Event reference.
use crate::{Actor, Core, CoreError};

/// Serializes publication with collection and remembers the next collection batch.
#[derive(Debug, Default)]
pub(crate) struct Publication {
	collection_cursor: String,
}

impl Core {
	/// Collects at most 256 abandoned payloads or interrupted staging files.
	/// Returns the removed count or a stable authorization/storage error.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "collection must fence publication while checking asynchronous reference transactions"
	)]
	pub async fn collect_artifacts(
		&self,
		actor: &Actor,
	) -> Result<u32, CoreError> {
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		drop(_access);
		let mut publication = self.artifact_publication.lock().await;
		let _file_lock =
			crate::artifact_files::publication_lock(self.run_home()).await?;
		let home = self.run_home();
		let now = self.clock.now();
		let after = publication.collection_cursor.clone();
		let candidates =
			crate::filesystem::blocking(move || candidates(&home, now, &after))
				.await??;
		let next = if candidates.len() == 256 {
			candidates.last().expect("full batch").1.clone()
		} else {
			String::new()
		};
		let mut removed = 0;
		let _access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		for (directory, name) in candidates {
			if self
				.store
				.read(async |tx| tx.artifact_referenced(&name).await)
				.await?
			{
				continue;
			}
			crate::filesystem::blocking(move || {
				rustix::fs::unlinkat(
					&directory,
					name.as_str(),
					rustix::fs::AtFlags::empty(),
				)
				.map_err(crate::artifact_files::io_error)?;
				directory
					.sync_all()
					.map_err(crate::artifact_files::io_error)
			})
			.await??;
			removed += 1;
		}
		publication.collection_cursor = next;
		Ok(removed)
	}
}
fn candidates(
	home: &std::path::Path,
	now: std::time::SystemTime,
	after: &str,
) -> Result<Vec<(std::fs::File, String)>, CoreError> {
	let directory = crate::artifact_files::directory(home)?;
	let mut names = std::collections::BTreeSet::new();
	for entry in rustix::fs::Dir::read_from(&directory)
		.map_err(crate::artifact_files::io_error)?
	{
		let entry = entry.map_err(crate::artifact_files::io_error)?;
		let Ok(name) = entry.file_name().to_str() else {
			continue;
		};
		if name <= after {
			continue;
		}
		if crate::artifact::validate_hash(name).is_err()
			&& !name
				.strip_prefix(".pending-")
				.is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
		{
			continue;
		}
		let Ok(file) = crate::artifact_files::open(&directory, name) else {
			continue;
		};
		// A slow upload can outlive the grace period; age alone is not abandonment.
		if name.starts_with(".pending-") && file.try_lock().is_err() {
			continue;
		}
		let modified = file
			.metadata()
			.and_then(|m| m.modified())
			.map_err(crate::artifact_files::io_error)?;
		if now
			.duration_since(modified)
			.is_ok_and(|age| age.as_secs() >= 86400)
		{
			names.insert(name.to_owned());
			if names.len() > 256 {
				names.pop_last();
			}
		}
	}
	names
		.into_iter()
		.map(|name| {
			Ok((
				directory
					.try_clone()
					.map_err(crate::artifact_files::io_error)?,
				name,
			))
		})
		.collect()
}
