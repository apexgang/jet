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
		input: tokio::sync::Mutex::new(None),
	});
	let mut done = state.done.subscribe();
	let mut tasks = tokio::task::JoinSet::new();
	let startup = tokio::time::sleep(TIMEOUT);
	tokio::pin!(startup);
	let mut awaiting_start = true;
	loop {
		tokio::select! {
			_ = done.changed() => break,
			_ = &mut startup, if awaiting_start => {
				awaiting_start = false;
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
	/// Standard input of a Harness launched as `Streaming`, held here so it
	/// outlives the connection that launched it and a reconnecting Craft
	/// keeps writing to the same Harness. Taking it closes the pipe.
	input: tokio::sync::Mutex<Option<native::NativeInput>>,
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
		versions: vec![ProtocolVersion { major: 1, minor: 3 }],
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
			input_mode,
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
					*input_mode,
					std::sync::Arc::clone(&state.spool),
					native::Control {
						stop: state.stop.subscribe(),
						stopped: state.stopped.clone(),
						signal: state.signal.subscribe(),
					},
				)
				.await
				{
					Ok(native_input) => {
						*state.input.lock().await = native_input;
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
		| HelperCommand::Input { .. }
		| HelperCommand::CloseInput
		| HelperCommand::Acknowledge { .. } => {
			return Err(std::io::Error::other("invalid helper command"));
		}
	}
	// One bounded reader task keeps a partially decoded frame alive while
	// source records and native input share this connection. Reading inside
	// the select! below would instead abandon a frame mid-decode.
	let (commands, mut requests) = tokio::sync::mpsc::channel(8);
	let reader_task = tokio::spawn(async move {
		while let Ok(command) = receive::<HelperCommand>(&mut reader).await {
			if commands.send(command).await.is_err() {
				break;
			}
		}
	});
	let replay = async {
		loop {
			// `next` is cancel safe: an unacknowledged record stays queued,
			// so losing this race to native input re-reads the same record.
			// It is polled first because a Craft that acknowledges the
			// terminal record closes immediately afterwards: the source
			// having ended must settle this connection, not the peer having
			// left, or the helper would never finish.
			let record = tokio::select! {
				biased;
				record = state.spool.next() => record?,
				request = requests.recv() => {
					deliver(state, negotiated.version.minor, request).await?;
					continue;
				}
			};
			let Some(record) = record else { break };
			send(&mut writer, &record).await?;
			// Native input may arrive before the acknowledgement it precedes.
			let source_offset = loop {
				let request = requests.recv().await;
				if let Some(HelperCommand::Acknowledge { source_offset }) =
					request
				{
					break source_offset;
				}
				deliver(state, negotiated.version.minor, request).await?;
			};
			state.spool.acknowledge(source_offset).await?;
		}
		state.done.send_replace(true);
		Ok(())
	};
	let result = tokio::select! {
		result = replay => result,
		_ = generation.changed() => Err(std::io::Error::other("replaced execution connection")),
	};
	reader_task.abort();
	result
}

/// Carry one native input request to the Harness this connection launched.
/// Nothing else belongs on the record stream, and the helper never inspects
/// or frames the bytes it writes.
async fn deliver(
	state: &Execution,
	minor: u32,
	request: Option<HelperCommand>,
) -> std::io::Result<()> {
	let Some(request) = request else {
		return Err(std::io::Error::other("helper connection closed"));
	};
	match request {
		HelperCommand::Input { text } if minor >= 3 => {
			// Release the guard before waiting on the Harness: a Harness that
			// has not drained its input must not also block closing it.
			let open = state.input.lock().await.clone();
			let Some(open) = open else {
				return Err(std::io::Error::other(
					"native input is not streaming",
				));
			};
			open.write(text).await
		}
		// Taking the writer closes the pipe, and the Harness reads end of
		// input. No signal is delivered and no retained source is released.
		HelperCommand::CloseInput if minor >= 3 => {
			if state.input.lock().await.take().is_none() {
				return Err(std::io::Error::other(
					"native input is not streaming",
				));
			}
			Ok(())
		}
		HelperCommand::Input { .. } | HelperCommand::CloseInput => {
			Err(std::io::Error::other("native input requires Helper 1.3"))
		}
		HelperCommand::Inspect
		| HelperCommand::Terminate { .. }
		| HelperCommand::Signal { .. }
		| HelperCommand::Recover { .. }
		| HelperCommand::Launch { .. }
		| HelperCommand::Acknowledge { .. } => {
			Err(std::io::Error::other("expected source acknowledgement"))
		}
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
