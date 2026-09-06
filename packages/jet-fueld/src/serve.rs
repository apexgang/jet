//! One owner-only helper endpoint for one execution. Disconnects leave native work alive.
use crate::{native, spool::Spool};
use jet_protocol::{
	Frame, FrameReader, FrameWriter, HelperCommand, HelperConfig, HelperEvent,
	HelperHello, HelperReady, Negotiation, ProtocolFamily, ProtocolOffer,
	ProtocolVersion, decode_control, encode_control,
};
use std::{
	os::unix::fs::{MetadataExt, PermissionsExt},
	path::Path,
};
use tokio::{
	io::{AsyncRead, AsyncWrite},
	net::{UnixListener, UnixStream},
	time::{Duration, timeout},
};

const TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) async fn serve(path: &Path) -> std::io::Result<()> {
	let directory = path
		.parent()
		.ok_or_else(|| std::io::Error::other("missing helper directory"))?;
	for target in [directory, path] {
		let metadata = std::fs::symlink_metadata(target)?;
		if metadata.file_type().is_symlink()
			|| metadata.uid() != rustix::process::getuid().as_raw()
			|| metadata.permissions().mode() & 0o077 != 0
		{
			return Err(std::io::Error::other(
				"helper configuration is not owner-only",
			));
		}
	}
	if std::fs::metadata(path)?.len() > 65_536 {
		return Err(std::io::Error::other("oversized helper configuration"));
	}
	let config: HelperConfig =
		decode_control(&std::fs::read(path)?).map_err(std::io::Error::other)?;
	let socket = directory.join("h.sock");
	let listener = UnixListener::bind(&socket)?;
	std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
	let spool = Spool::new(directory.to_path_buf());
	let mut launched = None;
	loop {
		let accepted = if launched.is_none() {
			timeout(TIMEOUT, listener.accept())
				.await
				.map_err(std::io::Error::other)?
		} else {
			listener.accept().await
		};
		let (stream, _) = accepted?;
		match connection(stream, &config, &spool, &mut launched).await {
			Ok(()) => {
				std::fs::remove_file(&socket)?;
				return Ok(());
			}
			Err(_) => { /* Keep the native process and its unacknowledged source. */
			}
		}
	}
}

async fn connection(
	stream: UnixStream,
	config: &HelperConfig,
	spool: &std::sync::Arc<Spool>,
	launched: &mut Option<Vec<u8>>,
) -> std::io::Result<()> {
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let hello: HelperHello = timeout(TIMEOUT, receive(&mut reader))
		.await
		.map_err(std::io::Error::other)??;
	let offer = ProtocolOffer {
		family: ProtocolFamily::Helper,
		versions: vec![ProtocolVersion { major: 1, minor: 0 }],
		capabilities: vec![],
	};
	let negotiated = offer
		.negotiate(&hello.protocol, Negotiation::NewExecution)
		.map_err(std::io::Error::other)?;
	if hello.execution_id != config.execution_id {
		return Err(std::io::Error::other("wrong execution"));
	}
	send(
		&mut writer,
		&HelperReady {
			version: negotiated.version,
			helper_pid: std::process::id(),
		},
	)
	.await?;
	let command: HelperCommand = timeout(TIMEOUT, receive(&mut reader))
		.await
		.map_err(std::io::Error::other)??;
	let request = encode_control(&command).map_err(std::io::Error::other)?;
	if let Some(previous) = launched {
		if *previous != request {
			return Err(std::io::Error::other("conflicting native launch"));
		}
	} else {
		let HelperCommand::Launch {
			program,
			arguments,
			input,
		} = command
		else {
			return Err(std::io::Error::other("expected native launch"));
		};
		// Record the attempt before spawn: a failed acknowledgement never triggers
		// an automatic second launch on this helper.
		*launched = Some(request);
		match native::launch(
			config,
			program,
			arguments,
			input,
			std::sync::Arc::clone(spool),
		)
		.await
		{
			Ok(()) => {}
			Err(native::LaunchError::NotStarted) => {
				spool.append(HelperEvent::LaunchFailed).await?
			}
			Err(native::LaunchError::Unknown) => {
				return Err(std::io::Error::other(
					"native launch outcome is unknown",
				));
			}
		}
	}
	while let Some(record) = spool.next().await? {
		send(&mut writer, &record).await?;
		let HelperCommand::Acknowledge { source_offset } =
			receive(&mut reader).await?
		else {
			return Err(std::io::Error::other(
				"expected source acknowledgement",
			));
		};
		spool.acknowledge(source_offset).await?;
	}
	Ok(())
}

async fn receive<T: serde::de::DeserializeOwned>(
	reader: &mut FrameReader<impl AsyncRead + Unpin>,
) -> std::io::Result<T> {
	match reader.read().await.map_err(std::io::Error::other)? {
		Frame::Control { stream_id, payload } if stream_id.is_connection() => {
			decode_control(&payload).map_err(std::io::Error::other)
		}
		Frame::Control { .. } | Frame::Data { .. } => {
			Err(std::io::Error::other("expected helper control"))
		}
	}
}
async fn send(
	writer: &mut FrameWriter<impl AsyncWrite + Unpin>,
	value: &impl serde::Serialize,
) -> std::io::Result<()> {
	let payload = encode_control(value).map_err(std::io::Error::other)?;
	timeout(TIMEOUT, writer.write(&Frame::control(payload)))
		.await
		.map_err(std::io::Error::other)?
		.map_err(std::io::Error::other)
}
