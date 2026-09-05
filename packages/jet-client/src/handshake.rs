//! Restricted negotiation shared by local IPC and authenticated SSH I/O.

use crate::{Client, ClientError};
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, ConnectionProof, Frame, FrameLimits,
	FrameReader, FrameWriter, PREFACE, PROTOCOL_MINOR, PROTOCOL_VERSION,
	REMOTE_AUTH_MINOR, ServerHello, VersionRange, connection_signing_bytes,
	decode_control, encode_control,
};
use std::future::Future;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

/// Installation-owned signing seam. Implementations resolve the private key
/// through platform credential storage; they never return or persist it in Jet.
pub trait ClientIdentity {
	/// The durable identity established during Pairing.
	fn client_id(&self) -> Uuid;
	/// Signs only the connection transcript supplied by Jet.
	fn sign(
		&self,
		transcript: &[u8],
	) -> impl Future<Output = std::io::Result<[u8; 64]>> + Send;
}

impl Client {
	/// Submits one restricted Pairing operation over authenticated SSH I/O.
	/// The response grants no application access; reconnect after confirmation
	/// and completion to perform the normal signed handshake.
	///
	/// # Errors
	/// Returns a transport, timeout, or stable Pairing refusal.
	pub async fn pair_remote<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
		read: R,
		write: W,
		client_id: Uuid,
		request: &jet_protocol::RemotePairingRequest,
	) -> Result<jet_protocol::RemotePairingResponse, ClientError> {
		tokio::time::timeout(Duration::from_secs(15), async {
			let (mut reader, mut writer) = begin(read, write, &hello(client_id)).await?;
			match receive(&mut reader).await? {
				ServerHello::Challenge { .. } => {},
				ServerHello::Rejected { error } => return Err(ClientError::Rejected(error)),
				ServerHello::Welcome { .. } => return Err(ClientError::Unexpected("endpoint skipped restricted Pairing".into())),
			}
			writer.write(&Frame::control(encode_control(request)?)).await?;
			match receive(&mut reader).await? {
				jet_protocol::RemotePairingResponse::Rejected { error } => Err(ClientError::Remote(error)),
				response @ (jet_protocol::RemotePairingResponse::Claimed { .. } | jet_protocol::RemotePairingResponse::Completed { .. }) => Ok(response),
			}
		}).await.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "remote Pairing timed out"))?
	}

	/// Authenticates over an already endpoint-authenticated SSH byte stream.
	/// Callers own that transport's lifetime. No Plane state is requested
	/// before the server accepts a fresh signature.
	///
	/// # Errors
	/// Returns a handshake, credential-store, transport, or timeout failure.
	pub async fn connect_remote<R, W>(
		read: R,
		write: W,
		identity: &impl ClientIdentity,
	) -> Result<Self, ClientError>
	where
		R: AsyncRead + Unpin + Send + 'static,
		W: AsyncWrite + Unpin + Send + 'static,
	{
		tokio::time::timeout(Duration::from_secs(15), async {
			let hello = hello(identity.client_id());
			let (mut reader, mut writer) = begin(read, write, &hello).await?;
			let nonce = match receive(&mut reader).await? {
				ServerHello::Challenge { nonce } => nonce,
				ServerHello::Rejected { error } => return Err(ClientError::Rejected(error)),
				ServerHello::Welcome { .. } => return Err(ClientError::Unexpected("remote endpoint skipped authentication".into())),
			};
			let signature = identity.sign(&connection_signing_bytes(&hello, &nonce)?).await?;
			writer.write(&Frame::control(encode_control(&ConnectionProof { signature })?)).await?;
			let welcome = receive(&mut reader).await?;
			if matches!(&welcome, ServerHello::Welcome { minor, .. } if *minor < REMOTE_AUTH_MINOR) {
				return Err(ClientError::Unexpected("remote endpoint downgraded authentication".into()));
			}
			Self::from_handshake(reader, writer, welcome)
		}).await.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "remote handshake timed out"))?
	}
}

pub(crate) async fn local<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	read: R,
	write: W,
	client_id: Uuid,
) -> Result<(FrameReader<R>, FrameWriter<W>, ServerHello), ClientError> {
	let (mut reader, writer) = begin(read, write, &hello(client_id)).await?;
	let welcome = receive(&mut reader).await?;
	Ok((reader, writer, welcome))
}

fn hello(client_id: Uuid) -> ClientHello {
	let limits = FrameLimits::default();
	ClientHello {
		protocol: VersionRange {
			min: PROTOCOL_VERSION,
			max: PROTOCOL_VERSION,
		},
		minor: PROTOCOL_MINOR,
		codec: CODEC_JSON_V1.into(),
		client_id,
		max_control_frame: u32::try_from(limits.control).unwrap_or(u32::MAX),
		max_data_frame: u32::try_from(limits.data).unwrap_or(u32::MAX),
		capabilities: vec![],
	}
}

async fn begin<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	read: R,
	mut write: W,
	hello: &ClientHello,
) -> Result<(FrameReader<R>, FrameWriter<W>), ClientError> {
	write.write_all(PREFACE).await?;
	let mut writer = FrameWriter::new(write);
	writer
		.write(&Frame::control(encode_control(hello)?))
		.await?;
	Ok((FrameReader::new(read), writer))
}

async fn receive<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
	reader: &mut FrameReader<R>,
) -> Result<T, ClientError> {
	match reader.read().await? {
		Frame::Control { stream_id, payload } if stream_id.is_connection() => {
			Ok(decode_control(&payload)?)
		}
		Frame::Control { .. } | Frame::Data { .. } => {
			Err(ClientError::Unexpected("invalid handshake frame".into()))
		}
	}
}
