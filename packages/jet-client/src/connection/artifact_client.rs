//! Bounded raw transfers beside ordinary request/reply traffic.
use super::*;
use jet_protocol::{
	ArtifactControl as Control, ArtifactDescriptor, ArtifactVerifier,
	Sha256Digest, StreamControl,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) type Transfers = Arc<Mutex<HashMap<StreamId, TransferReply>>>;
#[derive(Debug)]
pub(super) struct TransferReply {
	reply: mpsc::Sender<Frame>,
	_permit: OwnedSemaphorePermit,
}
struct Transfer<'a> {
	client: &'a Client,
	stream: StreamId,
	replies: mpsc::Receiver<Frame>,
	completed: bool,
}
impl Client {
	/// Streams an upload and returns only its verified, durably published identity.
	/// Retrying the same Run/hash is safe even after a lost completion reply.
	/// Returns declaration, integrity, protocol, or transport errors.
	pub async fn upload_artifact<R: AsyncRead + Unpin>(
		&self,
		run_id: Uuid,
		artifact: ArtifactDescriptor,
		source: &mut R,
	) -> Result<ArtifactDescriptor, ClientError> {
		let mut transfer = self
			.artifact_transfer(&Control::ArtifactUpload {
				run_id,
				artifact: artifact.clone(),
			})
			.await?;
		let mut verifier =
			ArtifactVerifier::new(artifact.size, artifact.sha256);
		loop {
			let frame = transfer.receive().await?;
			let StreamControl::Credit { bytes } = control(&frame)? else {
				return Err(unexpected());
			};
			if bytes == 0 || bytes > self.data_limit as u64 {
				return Err(unexpected());
			}
			let mut buffer = vec![0; bytes as usize];
			let read = source.read(&mut buffer).await?;
			if read == 0 {
				break;
			}
			buffer.truncate(read);
			verifier.accept(&buffer).map_err(|_| unexpected())?;
			self.send_frame(Frame::data(transfer.stream, buffer))
				.await?;
		}
		verifier.finish().map_err(|_| unexpected())?;
		self.send_on(transfer.stream, &Control::ArtifactCommit)
			.await?;
		let frame = transfer.receive().await?;
		match control(&frame)? {
			Control::ArtifactPublished {
				artifact: published,
			} if published == artifact => {
				transfer.completed = true;
				Ok(published)
			}
			_ => Err(unexpected()),
		}
	}

	/// Writes a download incrementally; only a successful return authenticates all bytes.
	/// Callers must discard their partial destination if this returns an error.
	/// Returns storage, integrity, protocol, or transport errors.
	pub async fn download_artifact<W: AsyncWrite + Unpin>(
		&self,
		sha256: Sha256Digest,
		destination: &mut W,
	) -> Result<ArtifactDescriptor, ClientError> {
		let mut transfer = self
			.artifact_transfer(&Control::ArtifactDownload { sha256 })
			.await?;
		let frame = transfer.receive().await?;
		let Control::ArtifactDownloading { artifact } = control(&frame)? else {
			return Err(unexpected());
		};
		if artifact.sha256 != sha256 {
			return Err(unexpected());
		}
		let mut verifier =
			ArtifactVerifier::new(artifact.size, artifact.sha256);
		let mut remaining = artifact.size;
		let mut credit = 0;
		loop {
			if credit == 0 && remaining > 0 {
				credit = remaining.min(self.data_limit.min(65536) as u64);
				self.send_on(
					transfer.stream,
					&StreamControl::Credit { bytes: credit },
				)
				.await?;
			}
			let frame = transfer.receive().await?;
			match frame {
				Frame::Data { payload, .. }
					if !payload.is_empty()
						&& payload.len() as u64 <= credit =>
				{
					verifier.accept(&payload).map_err(|_| unexpected())?;
					remaining -= payload.len() as u64;
					credit -= payload.len() as u64;
					destination.write_all(&payload).await?;
				}
				frame @ Frame::Control { .. } => {
					let StreamControl::ArtifactFinished {
						total_bytes,
						sha256,
					} = control(&frame)?
					else {
						return Err(unexpected());
					};
					if total_bytes != artifact.size || sha256 != artifact.sha256
					{
						return Err(unexpected());
					}
					verifier.finish().map_err(|_| unexpected())?;
					destination.flush().await?;
					transfer.completed = true;
					return Ok(artifact);
				}
				Frame::Data { .. } => return Err(unexpected()),
			}
		}
	}

	/// Runs one bounded grace-period collection batch and returns its removed count.
	/// Returns a daemon refusal or transport error.
	pub async fn collect_artifacts(&self) -> Result<u32, ClientError> {
		let mut transfer =
			self.artifact_transfer(&Control::ArtifactCollect).await?;
		let frame = transfer.receive().await?;
		let Control::ArtifactCollected { removed } = control(&frame)? else {
			return Err(unexpected());
		};
		transfer.completed = true;
		Ok(removed)
	}

	async fn artifact_transfer(
		&self,
		request: &Control,
	) -> Result<Transfer<'_>, ClientError> {
		self.require_minor(jet_protocol::ARTIFACTS_MINOR)?;
		let permit = Arc::clone(&self.in_flight)
			.acquire_owned()
			.await
			.map_err(|_| ClientError::Closed)?;
		let stream = self.request_stream();
		let (reply, replies) = mpsc::channel(4);
		self.transfers.lock().expect("Artifact replies").insert(
			stream,
			TransferReply {
				reply,
				_permit: permit,
			},
		);
		let transfer = Transfer {
			client: self,
			stream,
			replies,
			completed: false,
		};
		self.send_on(stream, request).await?;
		Ok(transfer)
	}
}
impl Transfer<'_> {
	async fn receive(&mut self) -> Result<Frame, ClientError> {
		let frame = self.replies.recv().await.ok_or(ClientError::Closed)?;
		if let Frame::Control { payload, .. } = &frame
			&& let Ok(ServerMessage::Error { error, .. }) =
				decode_control(payload)
		{
			self.completed = true;
			return Err(ClientError::Remote(error));
		}
		Ok(frame)
	}
}
impl Drop for Transfer<'_> {
	fn drop(&mut self) {
		if self.completed
			|| !self
				.client
				.transfers
				.lock()
				.expect("Artifact replies")
				.contains_key(&self.stream)
		{
			return;
		}
		// Keep routing late data until the cancellation reply releases the entry.
		let outbound = self.client.outbound.clone();
		let stream = self.stream;
		tokio::spawn(async move {
			if let Ok(payload) = encode_control(&Control::ArtifactCancel) {
				let (finished, _) = oneshot::channel();
				let _ = outbound
					.send(WriteRequest {
						frame: Frame::stream_control(stream, payload),
						finished,
					})
					.await;
			}
		});
	}
}
fn control<T: serde::de::DeserializeOwned>(
	frame: &Frame,
) -> Result<T, ClientError> {
	let Frame::Control { payload, .. } = frame else {
		return Err(unexpected());
	};
	Ok(decode_control(payload)?)
}
fn unexpected() -> ClientError {
	ClientError::Unexpected(
		"Artifact transfer violated its declaration or stream contract".into(),
	)
}

pub(super) fn route(transfers: &Transfers, frame: &Frame) -> Result<bool, ()> {
	let mut transfers = transfers.lock().expect("Artifact replies");
	let Some(transfer) = transfers.get(&frame.stream_id()) else {
		// Cancellation may race an already queued completion; both close the stream.
		return Ok(matches!(
			control::<Control>(frame),
			Ok(Control::ArtifactCanceled)
		));
	};
	match transfer.reply.try_send(frame.clone()) {
		Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
		Err(mpsc::error::TrySendError::Full(_)) => return Err(()),
	}
	if let Frame::Control { payload, .. } = frame {
		let completed = matches!(
			decode_control::<Control>(payload),
			Ok(Control::ArtifactPublished { .. }
				| Control::ArtifactCollected { .. }
				| Control::ArtifactCanceled)
		) || matches!(
			decode_control::<StreamControl>(payload),
			Ok(StreamControl::ArtifactFinished { .. })
		) || matches!(
			decode_control::<ServerMessage>(payload),
			Ok(ServerMessage::Error { .. })
		);
		if completed {
			transfers.remove(&frame.stream_id());
		}
	}
	Ok(true)
}
