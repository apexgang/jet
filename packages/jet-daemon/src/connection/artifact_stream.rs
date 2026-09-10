//! Per-connection bounded Artifact transfers with cumulative byte credit.
use jet_core::{Actor, Core};
use jet_protocol::{
	ArtifactControl as Control, ArtifactDescriptor, Frame, StreamControl,
	StreamId, WireError, encode_control,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc;

type Published = (StreamId, Result<ArtifactDescriptor, WireError>);

const MAX_CREDIT: u64 = 16 * 1024 * 1024;

pub(crate) struct Artifacts {
	streams: HashMap<StreamId, Transfer>,
	limit: u32,
	last_download: u32,
	publications: tokio::task::JoinSet<Published>,
}
enum Transfer {
	Publishing {
		respond: bool,
	},
	Upload {
		upload: jet_core::ArtifactUpload,
		credit: u64,
	},
	Download {
		download: jet_core::ArtifactDownload,
		remaining: u64,
		credit: u64,
	},
}
impl Artifacts {
	pub(crate) fn new(limit: u32) -> Self {
		Self {
			streams: HashMap::new(),
			limit,
			last_download: 0,
			publications: tokio::task::JoinSet::new(),
		}
	}
	pub(crate) fn contains(&self, stream: StreamId) -> bool {
		self.streams.contains_key(&stream)
	}

	pub(crate) async fn control(
		&mut self,
		stream: StreamId,
		control: Control,
		core: &Arc<Core>,
		actor: &Actor,
		minor: u32,
		replies: &mpsc::Sender<Frame>,
	) -> Result<(), WireError> {
		if minor < jet_protocol::ARTIFACTS_MINOR {
			return Err(crate::connection::wire_error(
				jet_protocol::ErrorCategory::Incompatible,
				"protocol.unsupported_minor",
				"Artifacts require protocol minor 31".into(),
			));
		}
		match control {
			Control::ArtifactUpload { run_id, artifact } => {
				self.admit(stream)?;
				let upload = core
					.begin_artifact_upload(
						actor,
						jet_core::RunId(run_id),
						jet_core::ArtifactDescriptor {
							sha256: artifact.sha256.to_string(),
							size: artifact.size,
						},
					)
					.await
					.map_err(|e| crate::translate::error(e, minor))?;
				self.streams.insert(
					stream,
					Transfer::Upload {
						upload,
						credit: self.limit.into(),
					},
				);
				send(
					replies,
					stream,
					&StreamControl::Credit {
						bytes: self.limit.into(),
					},
				)
				.await
			}
			Control::ArtifactDownload { sha256 } => {
				self.admit(stream)?;
				let download = core
					.artifact(actor, &sha256.to_string())
					.await
					.map_err(|e| crate::translate::error(e, minor))?;
				let artifact = descriptor(download.descriptor())?;
				let remaining = artifact.size;
				self.streams.insert(
					stream,
					Transfer::Download {
						download,
						remaining,
						credit: 0,
					},
				);
				send(
					replies,
					stream,
					&Control::ArtifactDownloading { artifact },
				)
				.await?;
				Ok(())
			}
			Control::ArtifactCommit => {
				if !matches!(
					self.streams.get(&stream),
					Some(Transfer::Upload { .. })
				) {
					return Err(invalid());
				}
				let Some(Transfer::Upload { upload, .. }) =
					self.streams.remove(&stream)
				else {
					unreachable!()
				};
				self.streams
					.insert(stream, Transfer::Publishing { respond: true });
				let core = Arc::clone(core);
				let actor = actor.clone();
				self.publications.spawn(async move {
					let result = core
						.publish_artifact(&actor, upload)
						.await
						.map_err(|e| crate::translate::error(e, minor))
						.and_then(|published| descriptor(&published));
					(stream, result)
				});
				Ok(())
			}
			Control::ArtifactCancel => {
				self.discard(stream);
				send(replies, stream, &Control::ArtifactCanceled).await
			}
			Control::ArtifactCollect => {
				if self.contains(stream) {
					return Err(invalid());
				}
				let removed = core
					.collect_artifacts(actor)
					.await
					.map_err(|e| crate::translate::error(e, minor))?;
				send(replies, stream, &Control::ArtifactCollected { removed })
					.await
			}
			Control::ArtifactPublished { .. }
			| Control::ArtifactDownloading { .. }
			| Control::ArtifactCanceled
			| Control::ArtifactCollected { .. } => Err(invalid()),
		}
	}
	fn admit(&self, stream: StreamId) -> Result<(), WireError> {
		if stream.is_connection()
			|| self.contains(stream)
			|| self.streams.len() >= 4
		{
			return Err(invalid());
		}
		Ok(())
	}
	pub(crate) async fn input(
		&mut self,
		stream: StreamId,
		bytes: &[u8],
		minor: u32,
		replies: &mpsc::Sender<Frame>,
	) -> Result<(), WireError> {
		let Some(Transfer::Upload { upload, credit }) =
			self.streams.get_mut(&stream)
		else {
			return Err(invalid());
		};
		let count = bytes.len() as u64;
		if count > *credit || bytes.is_empty() {
			return Err(invalid());
		}
		*credit -= count;
		upload
			.write_chunk(bytes)
			.await
			.map_err(|e| crate::translate::error(e, minor))?;
		send(replies, stream, &StreamControl::Credit { bytes: count }).await?;
		*credit += count;
		Ok(())
	}
	pub(crate) fn credit(
		&mut self,
		stream: StreamId,
		bytes: u64,
	) -> Result<(), WireError> {
		let Some(Transfer::Download { credit, .. }) =
			self.streams.get_mut(&stream)
		else {
			return Err(invalid());
		};
		*credit = credit
			.checked_add(bytes)
			.filter(|sum| *sum <= MAX_CREDIT)
			.ok_or_else(invalid)?;
		Ok(())
	}
	pub(crate) fn next_download(&self) -> Option<StreamId> {
		self.streams
			.iter()
			.filter_map(|(stream, transfer)| match transfer {
				Transfer::Download {
					remaining, credit, ..
				} if *remaining == 0 || *credit > 0 => Some(*stream),
				Transfer::Download { .. }
				| Transfer::Upload { .. }
				| Transfer::Publishing { .. } => None,
			})
			.min_by_key(|stream| {
				(stream.get() <= self.last_download, stream.get())
			})
	}
	pub(crate) async fn write_download_chunk(
		&mut self,
		stream: StreamId,
		minor: u32,
		replies: &mpsc::Sender<Frame>,
	) -> Result<(), WireError> {
		self.last_download = stream.get();
		let Some(Transfer::Download {
			download,
			remaining,
			credit,
		}) = self.streams.get_mut(&stream)
		else {
			return Err(invalid());
		};
		// ASVS 15.2.2: consume cumulative credit one bounded chunk at a time;
		// the request loop gives control requests priority between chunks.
		if *remaining != 0 {
			let chunk = download
				.read_chunk((*credit).min(self.limit.into()) as usize)
				.await
				.map_err(|e| crate::translate::error(e, minor))?;
			*remaining -= chunk.len() as u64;
			*credit -= chunk.len() as u64;
			replies
				.send(Frame::data(stream, chunk))
				.await
				.map_err(|_| invalid())?;
		}
		if *remaining == 0 {
			let Some(Transfer::Download { download, .. }) =
				self.streams.remove(&stream)
			else {
				unreachable!()
			};
			let artifact = descriptor(download.descriptor())?;
			download
				.finish()
				.map_err(|e| crate::translate::error(e, minor))?;
			send(
				replies,
				stream,
				&StreamControl::ArtifactFinished {
					total_bytes: artifact.size,
					sha256: artifact.sha256,
				},
			)
			.await?;
		}
		Ok(())
	}
	pub(crate) fn has_publication(&self) -> bool {
		!self.publications.is_empty()
	}
	pub(crate) async fn publication(
		&mut self,
	) -> Result<Published, tokio::task::JoinError> {
		self.publications
			.join_next()
			.await
			.expect("pending publication")
	}
	pub(crate) async fn finish_publication(
		&mut self,
		(stream, result): Published,
		replies: &mpsc::Sender<Frame>,
	) -> Result<(), WireError> {
		if matches!(
			self.streams.remove(&stream),
			Some(Transfer::Publishing { respond: true })
		) {
			send(
				replies,
				stream,
				&Control::ArtifactPublished { artifact: result? },
			)
			.await?;
		}
		Ok(())
	}
	pub(crate) fn discard(&mut self, stream: StreamId) {
		if let Some(Transfer::Publishing { respond }) =
			self.streams.get_mut(&stream)
		{
			// Commit may already be durable. Suppress its late reply, but retain
			// the concurrency slot until the publication operation has finished.
			*respond = false;
		} else {
			self.streams.remove(&stream);
		}
	}
}
fn descriptor(
	value: &jet_core::ArtifactDescriptor,
) -> Result<ArtifactDescriptor, WireError> {
	Ok(ArtifactDescriptor {
		sha256: jet_protocol::Sha256Digest::parse(&value.sha256)
			.map_err(|_| invalid())?,
		size: value.size,
	})
}
async fn send(
	replies: &mpsc::Sender<Frame>,
	stream: StreamId,
	value: &impl serde::Serialize,
) -> Result<(), WireError> {
	let payload = encode_control(value).map_err(|_| invalid())?;
	replies
		.send(Frame::stream_control(stream, payload))
		.await
		.map_err(|_| invalid())
}
pub(crate) fn invalid() -> WireError {
	crate::connection::wire_error(
		jet_protocol::ErrorCategory::InvalidInput,
		"artifact.invalid_stream",
		"the Artifact stream violated its state, credit, or concurrency limit"
			.into(),
	)
}
