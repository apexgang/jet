//! Bounded grace-period collection without touching authoritative state.
use crate::{Actor, Core, CoreError, filesystem::disposable};

/// Serializes publication with collection and remembers each namespace's batch.
#[derive(Debug, Default)]
pub(crate) struct Publication {
	collection_cursor: [String; 2],
}

impl Core {
	/// Collects at most 64 entries per disposable namespace after a 24-hour grace.
	/// References and active file locks always protect content, including under pressure.
	/// Returns the removed count or a stable authorization/storage error.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "collection fences publication until reference checks and unlink complete"
	)]
	pub async fn collect_artifacts(
		&self,
		actor: &Actor,
	) -> Result<u32, CoreError> {
		let access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		drop(access);
		let mut publication = self.artifact_publication.lock().await;
		let _file_lock =
			crate::artifact::files::publication_lock(self.run_home()).await?;
		let access =
			self.remote_access.acquire().await.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		let mut removed = 0;
		for (index, folder) in disposable::FOLDERS.into_iter().enumerate() {
			let home = self.run_home();
			let after = publication.collection_cursor[index].clone();
			let page = crate::filesystem::blocking(move || {
				disposable::batch(&home, folder, &after)
			})
			.await??;
			for entry in page.entries {
				if entry.active {
					continue;
				}
				if folder == disposable::Namespace::Payloads
					&& self
						.store
						.read(async |tx| {
							tx.artifact_referenced(&entry.name).await
						})
						.await?
				{
					continue;
				}
				let now = self.clock.now();
				removed += crate::filesystem::blocking(move || {
					let modified = entry
						.file
						.metadata()
						.and_then(|m| m.modified())
						.map_err(crate::artifact::files::io_error)?;
					if !now
						.duration_since(modified)
						.is_ok_and(|age| age.as_secs() >= 86400)
					{
						return Ok::<_, CoreError>(0);
					}
					rustix::fs::unlinkat(
						&entry.directory,
						entry.name.as_str(),
						rustix::fs::AtFlags::empty(),
					)
					.map_err(crate::artifact::files::io_error)?;
					entry
						.directory
						.sync_all()
						.map_err(crate::artifact::files::io_error)?;
					Ok(1)
				})
				.await??;
			}
			publication.collection_cursor[index] = page.next;
		}
		drop(access);
		Ok(removed)
	}
}
