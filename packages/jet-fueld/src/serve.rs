//! One owner-only helper endpoint for one execution. Disconnects leave native work alive.
use crate::{native, spool::Spool};
use jet_protocol::{
	Frame, FrameReader, FrameWriter, HelperCommand, HelperConfig, HelperEvent,
	HelperHello, HelperReady, HelperSignalled, NativeSignal, Negotiation,
	ProtocolFamily, ProtocolOffer, ProtocolVersion, decode_control,
	encode_control,
};
use std::{os::unix::fs::PermissionsExt, path::Path};
use tokio::{
	io::{AsyncRead, AsyncWrite},
	net::{UnixListener, UnixStream},
	time::{Duration, timeout},
};

const TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) async fn serve(path: &Path) -> std::io::Result<()> {
	use std::os::unix::fs::OpenOptionsExt;
	let directory = path
		.parent()
		.ok_or_else(|| std::io::Error::other("missing helper directory"))?;
	let config: HelperConfig =
		decode_control(&jet_runtime::read_execution_file(path)?)
			.map_err(std::io::Error::other)?;
	let descriptor = jet_protocol::HelperDescriptor {
		replay: jet_protocol::HelperReplay::default(),
		role: "run".into(),
		config: config.clone(),
		instance: uuid::Uuid::new_v4(),
		pid: std::process::id(),
		process_start: jet_runtime::execution_process_identity(
			std::process::id(),
		)?
		.ok_or_else(|| std::io::Error::other("missing process identity"))?,
		version: env!("CARGO_PKG_VERSION").into(),
		sha256: jet_runtime::execution_digest(&std::env::current_exe()?)?,
	};
	let socket = directory.join("h.sock");
	let listener = UnixListener::bind(&socket)?;
	std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
	let temporary = directory.join("descriptor.tmp");
	let mut file = std::fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.open(&temporary)?;
	std::io::Write::write_all(
		&mut file,
		&encode_control(&descriptor).map_err(std::io::Error::other)?,
	)?;
	file.sync_all()?;
	std::fs::rename(temporary, directory.join("descriptor.json"))?;
	std::fs::File::open(directory)?.sync_all()?;
	let spool = Spool::new(directory.to_path_buf(), descriptor.clone());
	let state = std::sync::Arc::new(Execution {
		descriptor,
		spool,
		launched: tokio::sync::Mutex::new(None),
		generation: tokio::sync::watch::channel(0u64).0,
		done: tokio::sync::watch::channel(false).0,
		stop: tokio::sync::watch::channel(false).0,
		stopped: tokio::sync::watch::channel(false).0,
		started: tokio::sync::watch::channel(false).0,
		signal: tokio::sync::watch::channel(None).0,
	});
	let mut done = state.done.subscribe();
	let mut tasks = tokio::task::JoinSet::new();
	let mut startup = tokio::time::interval(TIMEOUT);
	startup.tick().await;
	loop {
		tokio::select! {
			_ = done.changed() => break,
			_ = startup.tick() => {
				if let Ok(launched) = state.launched.try_lock() && launched.is_none() { break; }
			},
			Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
			accepted = listener.accept() => {
				let (stream, _) = accepted?;
				let Ok(peer) = stream.peer_cred() else { continue; };
				if peer.uid() != rustix::process::geteuid().as_raw() || tasks.len() >= 16 { continue; }
				let state = std::sync::Arc::clone(&state);
				tasks.spawn(async move { let _ = connection(stream, &state).await; });
			}
		}
	}
	std::fs::remove_file(&socket)?;
	// The helper removes its descriptor only after all source was acknowledged.
	std::fs::remove_file(directory.join("descriptor.json"))?;
	// Release only this execution's retained image, never the installed helper.
	let retained = directory.canonicalize()?.join("jetfueld");
	if std::env::current_exe()?.canonicalize()? == retained {
		std::fs::remove_file(retained)?;
	}
	Ok(())
}

struct Execution {
	descriptor: jet_protocol::HelperDescriptor,
	spool: std::sync::Arc<Spool>,
	launched: tokio::sync::Mutex<Option<Vec<u8>>>,
	generation: tokio::sync::watch::Sender<u64>,
	done: tokio::sync::watch::Sender<bool>,
	stop: tokio::sync::watch::Sender<bool>,
	stopped: tokio::sync::watch::Sender<bool>,
	/// Whether a native process was ever spawned. Only a live child can be
	/// signalled, and only through the group the helper itself created.
	started: tokio::sync::watch::Sender<bool>,
	/// The newest escalation step, sequenced so an identical repeat is
	/// still delivered.
	signal: tokio::sync::watch::Sender<Option<(u64, NativeSignal)>>,
}

#[expect(
	clippy::await_holding_invalid_type,
	reason = "one execution stream owns acknowledgements and replacement wakes the previous guard holder"
)]
async fn connection(
	stream: UnixStream,
	state: &Execution,
) -> std::io::Result<()> {
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let hello: HelperHello = timeout(TIMEOUT, receive(&mut reader))
		.await
		.map_err(std::io::Error::other)??;
	let offer = ProtocolOffer {
		family: ProtocolFamily::Helper,
		versions: vec![ProtocolVersion { major: 1, minor: 2 }],
		capabilities: vec![],
	};
	let negotiated = offer
		.negotiate(&hello.protocol, Negotiation::NewExecution)
		.map_err(std::io::Error::other)?;
	if hello.execution_id != state.descriptor.config.execution_id {
		return Err(std::io::Error::other("wrong execution"));
	}
	send(
		&mut writer,
		&HelperReady {
			version: negotiated.version,
			helper_pid: std::process::id(),
			descriptor: state.spool.descriptor(),
		},
	)
	.await?;
	let command: HelperCommand = timeout(TIMEOUT, receive(&mut reader))
		.await
		.map_err(std::io::Error::other)??;
	if matches!(command, HelperCommand::Inspect) {
		return Ok(());
	}
	if let HelperCommand::Signal { instance, signal } = command {
		// Signalling changes no source and ends no helper, so it never takes
		// the acknowledgement generation from the live execution stream.
		if negotiated.version.minor < 2
			|| instance != state.descriptor.instance
			|| !*state.started.borrow()
			|| *state.stop.borrow()
		{
			return Err(std::io::Error::other("wrong signal identity"));
		}
		state.signal.send_modify(|current| {
			let sequence = current.map_or(0, |(sequence, _)| sequence) + 1;
			*current = Some((sequence, signal));
		});
		send(&mut writer, &HelperSignalled { instance, signal }).await?;
		return Ok(());
	}
	if let HelperCommand::Terminate { instance } = command {
		if negotiated.version.minor < 1 || instance != state.descriptor.instance
		{
			return Err(std::io::Error::other("wrong termination identity"));
		}
		state.stop.send_replace(true);
		if let Ok(launched) = state.launched.try_lock()
			&& launched.is_none()
		{
			state.stopped.send_replace(true);
		}
		let mut stopped = state.stopped.subscribe();
		timeout(TIMEOUT, async {
			while !*stopped.borrow_and_update() {
				stopped.changed().await.map_err(std::io::Error::other)?;
			}
			Ok::<_, std::io::Error>(())
		})
		.await
		.map_err(std::io::Error::other)??;
		send(&mut writer, &jet_protocol::HelperTerminated { instance }).await?;
		state.done.send_replace(true);
		return Ok(());
	}
	if *state.stop.borrow() {
		return Err(std::io::Error::other("execution terminating"));
	}

	if let HelperCommand::Recover { source_offset } = command {
		if negotiated.version.minor < 1 {
			return Err(std::io::Error::other("recovery requires Helper 1.1"));
		}
		state.spool.validate_offset(source_offset)?;
	}
	// One stream owns acknowledgements. A validated replacement wakes the old
	// stream even while its Harness is silent or blocked on a full spool.
	state.generation.send_modify(|generation| *generation += 1);
	let mut generation = state.generation.subscribe();
	let mut launched = state.launched.lock().await;
	if generation.has_changed().unwrap_or(true) {
		return Err(std::io::Error::other("superseded connection"));
	}
	match &command {
		HelperCommand::Recover { source_offset }
			if negotiated.version.minor >= 1 && launched.is_some() =>
		{
			state.spool.recover(*source_offset).await?;
		}
		HelperCommand::Launch {
			program,
			arguments,
			input,
		} => {
			let request =
				encode_control(&command).map_err(std::io::Error::other)?;
			if let Some(previous) = &*launched {
				if *previous != request {
					return Err(std::io::Error::other(
						"conflicting native launch",
					));
				}
			} else {
				*launched = Some(request);
				match native::launch(
					&state.descriptor.config,
					program.clone(),
					arguments.clone(),
					input.clone(),
					std::sync::Arc::clone(&state.spool),
					native::Control {
						stop: state.stop.subscribe(),
						stopped: state.stopped.clone(),
						signal: state.signal.subscribe(),
					},
				)
				.await
				{
					Ok(()) => {
						state.started.send_replace(true);
					}
					Err(native::LaunchError::NotStarted) => {
						state.stopped.send_replace(true);
						state.spool.append(HelperEvent::LaunchFailed).await?;
					}
					Err(native::LaunchError::Unknown) => {
						return Err(std::io::Error::other(
							"native launch outcome is unknown",
						));
					}
				}
			}
		}
		HelperCommand::Inspect
		| HelperCommand::Terminate { .. }
		| HelperCommand::Signal { .. }
		| HelperCommand::Recover { .. }
		| HelperCommand::Acknowledge { .. } => {
			return Err(std::io::Error::other("invalid helper command"));
		}
	}
	let replay = async {
		while let Some(record) = state.spool.next().await? {
			send(&mut writer, &record).await?;
			let HelperCommand::Acknowledge { source_offset } =
				receive(&mut reader).await?
			else {
				return Err(std::io::Error::other(
					"expected source acknowledgement",
				));
			};
			state.spool.acknowledge(source_offset).await?;
		}
		state.done.send_replace(true);
		Ok(())
	};
	tokio::select! {
		result = replay => result,
		_ = generation.changed() => Err(std::io::Error::other("replaced execution connection")),
	}
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
