//! Bounded Craft control; private host-owned pipes establish the caller.
use jet_protocol::{
	CraftCommand, CraftEvent, CraftHello, CraftReady, CraftSpecification,
	Frame, FrameReader, FrameWriter, NegotiatedProtocol, Negotiation,
	ProtocolFamily, ProtocolOffer, ProtocolVersion, decode_control,
	encode_control,
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use tokio::{
	io::{AsyncRead, AsyncReadExt, AsyncWrite},
	time::timeout,
};

const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Stable SDK failure. Native payloads and parser diagnostics stay private.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum CraftError {
	/// The peer or specification cannot honor the execution's contract.
	#[error("incompatible Craft protocol or declarations")]
	Incompatible,
	/// The peer sent an invalid frame, message, or unsupported action.
	#[error("invalid Craft message")]
	InvalidMessage,
	/// The transport closed or failed; pending outcomes need reconciliation.
	#[error("Craft transport disconnected")]
	Disconnected,
	/// Startup or sending exceeded the bounded peer wait.
	#[error("Craft transport timed out")]
	Timeout,
}

/// One negotiated execution. Owns no processes, credentials, or core state.
/// Callers close it on any error; receive futures must not be canceled and
/// reused because cancellation can leave a partially consumed frame.
pub struct CraftConnection<R, W> {
	reader: FrameReader<R>,
	writer: FrameWriter<W>,
	hello: CraftHello,
	ready: CraftReady,
}

/// Command receive half, independently awaitable while native output is sent.
pub struct CraftReceiver<R> {
	reader: FrameReader<R>,
	ready: CraftReady,
}

/// Native output send half; awaiting sends supplies bounded backpressure.
pub struct CraftSender<W> {
	minor: u32,
	writer: FrameWriter<W>,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> CraftConnection<R, W> {
	/// Negotiate before returning a connection usable by a Harness adapter.
	/// Host-owned stdin/stdout or a private byte stream supply the transport.
	///
	/// # Errors
	/// Refuses incompatible declarations, invalid startup, and slow peers.
	pub async fn accept(
		mut read: R,
		write: W,
		specification: CraftSpecification,
	) -> Result<Self, CraftError> {
		timeout(IO_TIMEOUT, async {
			let mut preface = [0; 10];
			read.read_exact(&mut preface)
				.await
				.map_err(|_| CraftError::Disconnected)?;
			if &preface != b"jet-craft\n" {
				return Err(CraftError::InvalidMessage);
			}
			let mut reader = FrameReader::new(read);
			let mut writer = FrameWriter::new(write);
			let result = handshake(&mut reader, specification).await;
			let (hello, ready) = match result {
				Ok(result) => result,
				Err(error) => {
					#[derive(Serialize)]
					struct Rejected {
						kind: &'static str,
						code: CraftError,
					}
					let _ = send(
						&mut writer,
						&Rejected {
							kind: "rejected",
							code: error,
						},
					)
					.await;
					return Err(error);
				}
			};
			send(&mut writer, &ready).await?;
			reader.enable_multiplexing();
			writer.enable_multiplexing();
			Ok(Self {
				reader,
				writer,
				hello,
				ready,
			})
		})
		.await
		.map_err(|_| CraftError::Timeout)?
	}

	/// Host-supplied execution identity and explicit recovery or fork context.
	pub fn hello(&self) -> &CraftHello {
		&self.hello
	}

	/// Selected protocol remains unchanged for this connection's lifetime.
	pub fn negotiated(&self) -> &NegotiatedProtocol {
		&self.ready.protocol
	}

	/// Separate input and output so adapters can forward asynchronous native
	/// events while a Command receive remains pending, without canceling it.
	pub fn split(self) -> (CraftReceiver<R>, CraftSender<W>) {
		let minor = self.ready.protocol.version.minor;
		(
			CraftReceiver {
				reader: self.reader,
				ready: self.ready,
			},
			CraftSender {
				minor,
				writer: self.writer,
			},
		)
	}
}

impl<R: AsyncRead + Unpin> CraftReceiver<R> {
	/// Wait for the next admitted Command, with bounded allocation and no queue.
	///
	/// # Errors
	/// Unknown Commands, unsupported features, malformed frames, and disconnects
	/// fail without invoking the Harness. The caller must close the connection.
	pub async fn receive(&mut self) -> Result<CraftCommand, CraftError> {
		let command = receive(&mut self.reader).await?;
		let feature = match &command {
			CraftCommand::ConstrainSubagents { max_children } => {
				if self.ready.protocol.version.minor < 9
					|| !matches!(max_children, None | Some(0))
				{
					return Err(CraftError::InvalidMessage);
				}
				"subagents_limit"
			}
			CraftCommand::ConfigureRemoteTools { .. }
			| CraftCommand::RemoteToolResult { .. } => {
				if self.ready.protocol.version.minor < 6 {
					return Err(CraftError::InvalidMessage);
				}
				"remote_tools"
			}
			CraftCommand::Start { .. } | CraftCommand::Acknowledge { .. } => {
				if self.ready.protocol.version.minor < 1
					|| !self
						.ready
						.protocol
						.capabilities
						.iter()
						.any(|c| c == "runs")
				{
					return Err(CraftError::InvalidMessage);
				}
				"turns"
			}
			CraftCommand::Recover { .. } => {
				if self.ready.protocol.version.minor < 2 {
					return Err(CraftError::InvalidMessage);
				}
				"turns"
			}
			CraftCommand::Interrupt { .. } => {
				if self.ready.protocol.version.minor < 4 {
					return Err(CraftError::InvalidMessage);
				}
				"turns"
			}
			CraftCommand::Turn { .. } => "turns",
			CraftCommand::Action { .. } => "actions",
			CraftCommand::Shutdown => return Ok(command),
		};
		if !self
			.ready
			.enabled_features
			.iter()
			.any(|name| name == feature)
			|| (feature == "actions"
				&& !self
					.ready
					.protocol
					.capabilities
					.iter()
					.any(|name| name == feature))
		{
			return Err(CraftError::InvalidMessage);
		}
		Ok(command)
	}
}

impl<W: AsyncWrite + Unpin> CraftSender<W> {
	/// Send one complete native event or completion. Awaiting applies backpressure.
	///
	/// # Errors
	/// Rejects oversized or malformed output and closes on a slow/disconnected peer.
	pub async fn send(&mut self, event: &CraftEvent) -> Result<(), CraftError> {
		if self.minor < 7 && matches!(event, CraftEvent::Usage { .. }) {
			return Err(CraftError::InvalidMessage);
		}
		if self.minor < 6 && matches!(event, CraftEvent::RemoteTool { .. }) {
			return Err(CraftError::InvalidMessage);
		}
		if self.minor < 3
			&& matches!(
				event,
				CraftEvent::TurnStarted
					| CraftEvent::TurnEnded { .. }
					| CraftEvent::FileChanged { .. }
			) {
			return Err(CraftError::InvalidMessage);
		}
		if self.minor < 5
			&& matches!(
				event,
				CraftEvent::ConversationTitle { .. }
					| CraftEvent::RunTitle { .. }
					| CraftEvent::ProcessTitle { .. }
			) {
			return Err(CraftError::InvalidMessage);
		}
		if self.minor < 8
			&& matches!(event, CraftEvent::ApprovalRequested { .. })
		{
			return Err(CraftError::InvalidMessage);
		}
		send(&mut self.writer, event).await
	}
}

async fn handshake<R: AsyncRead + Unpin>(
	reader: &mut FrameReader<R>,
	specification: CraftSpecification,
) -> Result<(CraftHello, CraftReady), CraftError> {
	let hello: CraftHello = receive(reader).await?;
	let enabled_features = specification
		.enabled_features()
		.map_err(|_| CraftError::Incompatible)?;
	let schema = ProtocolOffer {
		family: ProtocolFamily::Specification,
		versions: vec![ProtocolVersion { major: 1, minor: 0 }],
		capabilities: vec![],
	};
	let specification_protocol = schema
		.negotiate(&hello.specification, Negotiation::NewExecution)
		.map_err(|_| CraftError::Incompatible)?;
	if hello.resume.is_some() && hello.fork.is_some() {
		return Err(CraftError::Incompatible);
	}
	let mode = hello
		.resume
		.as_ref()
		.map_or(Negotiation::NewExecution, |resume| {
			Negotiation::Resume(resume.version)
		});
	// ASVS 2.3.1: a specification cannot make this SDK speak a new codec major.
	let sdk = ProtocolOffer {
		family: ProtocolFamily::Craft,
		versions: vec![ProtocolVersion { major: 1, minor: 9 }],
		capabilities: vec![
			"actions".into(),
			"fork".into(),
			"resume".into(),
			"runs".into(),
		],
	};
	let supported = sdk
		.negotiate(&specification.protocol, mode)
		.map_err(|_| CraftError::Incompatible)?;
	let offer = ProtocolOffer {
		family: supported.family,
		versions: vec![supported.version],
		capabilities: supported.capabilities,
	};
	let protocol = offer
		.negotiate(&hello.protocol, mode)
		.map_err(|_| CraftError::Incompatible)?;
	if let Some(resume) = &hello.resume
		&& (resume.native_conversation.is_empty()
			|| !enabled_features.iter().any(|name| name == "resume")
			|| !protocol.capabilities.iter().any(|name| name == "resume"))
	{
		return Err(CraftError::Incompatible);
	}
	if let Some(fork) = &hello.fork
		&& (protocol.version.minor < 4
			|| fork.source_native_conversation.is_empty()
			|| fork.source_native_conversation.len() > 4096
			|| fork.checkpoint_turn == 0
			|| !git_object(&fork.checkpoint_commit)
			|| !git_object(&fork.checkpoint_tree)
			|| !enabled_features.iter().any(|name| name == "fork")
			|| !protocol.capabilities.iter().any(|name| name == "fork"))
	{
		return Err(CraftError::Incompatible);
	}
	Ok((
		hello,
		CraftReady {
			protocol,
			specification_protocol,
			specification,
			enabled_features,
		},
	))
}

fn git_object(value: &str) -> bool {
	(40..=64).contains(&value.len())
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn receive<R: AsyncRead + Unpin, T: DeserializeOwned>(
	reader: &mut FrameReader<R>,
) -> Result<T, CraftError> {
	// ASVS 1.5.2, 2.2.1: use the shared byte/depth/collection bounds and
	// accept only connection control; no implicit binary streams are open.
	match reader.read().await.map_err(|_| CraftError::Disconnected)? {
		Frame::Control { stream_id, payload } if stream_id.is_connection() => {
			decode_control(&payload).map_err(|_| CraftError::InvalidMessage)
		}
		Frame::Control { .. } | Frame::Data { .. } => {
			Err(CraftError::InvalidMessage)
		}
	}
}

async fn send<W: AsyncWrite + Unpin, T: Serialize>(
	writer: &mut FrameWriter<W>,
	message: &T,
) -> Result<(), CraftError> {
	let payload =
		encode_control(message).map_err(|_| CraftError::InvalidMessage)?;
	// Also validate locally generated collections before writing to the peer.
	decode_control::<serde::de::IgnoredAny>(&payload)
		.map_err(|_| CraftError::InvalidMessage)?;
	timeout(IO_TIMEOUT, writer.write(&Frame::control(payload)))
		.await
		.map_err(|_| CraftError::Timeout)?
		.map_err(|_| CraftError::Disconnected)
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
