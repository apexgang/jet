//! A terminal-role helper owns the PTY across every daemon/GUI disconnect.
use jet_protocol::*;
use jet_runtime::TerminalSpool;
use std::{
	io::{self, Read, Write},
	os::unix::fs::{OpenOptionsExt, PermissionsExt},
	path::Path,
	sync::{Arc, Mutex},
};
use tokio::{
	net::{UnixListener, UnixStream},
	time::{Duration, timeout},
};

const TIMEOUT: Duration = Duration::from_secs(10);
struct Terminal {
	descriptor: TerminalDescriptor,
	pty: Mutex<jet_runtime::TerminalPty>,
	input: Mutex<Box<dyn Write + Send>>,
	spool: Arc<Mutex<TerminalSpool>>,
	done: tokio::sync::Notify,
	closing: std::sync::atomic::AtomicBool,
	output_done: std::sync::atomic::AtomicBool,
}
pub(crate) async fn serve(path: &Path) -> io::Result<()> {
	let directory = path.parent().ok_or_else(invalid)?;
	let config: TerminalConfig =
		decode_control(&jet_runtime::read_execution_file(path)?)
			.map_err(io::Error::other)?;
	if config.boot != jet_runtime::execution_boot_identity()?
		|| Path::new(&config.root).canonicalize()? != Path::new(&config.root)
	{
		return Err(invalid());
	}
	let spool = Arc::new(Mutex::new(TerminalSpool::create(directory)?));
	let (pty, pipes) = jet_runtime::TerminalPty::open(
		Path::new(&config.root),
		&config.shell,
		config.rows,
		config.columns,
	)?;
	let descriptor = TerminalDescriptor {
		protocol: ProtocolVersion { major: 1, minor: 0 },
		role: "terminal".into(),
		config,
		instance: uuid::Uuid::new_v4(),
		pid: std::process::id(),
		process_start: jet_runtime::execution_process_identity(
			std::process::id(),
		)?
		.ok_or_else(invalid)?,
		sha256: jet_runtime::execution_digest(&std::env::current_exe()?)?,
		version: env!("CARGO_PKG_VERSION").into(),
	};
	let terminal = Arc::new(Terminal {
		descriptor,
		pty: Mutex::new(pty),
		input: Mutex::new(pipes.writer),
		spool,
		closing: std::sync::atomic::AtomicBool::new(false),
		output_done: std::sync::atomic::AtomicBool::new(false),
		done: tokio::sync::Notify::new(),
	});
	let socket = directory.join("h.sock");
	let listener = UnixListener::bind(&socket)?;
	std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
	let temporary = directory.join("descriptor.pending");
	let mut file = std::fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.open(&temporary)?;
	file.write_all(
		&encode_control(&terminal.descriptor).map_err(io::Error::other)?,
	)?;
	file.sync_all()?;
	std::fs::rename(temporary, directory.join("descriptor.json"))?;
	std::fs::File::open(directory)?.sync_all()?;
	let output = Arc::clone(&terminal);
	std::thread::spawn(move || pump(pipes.reader, &output));
	let mut tasks = tokio::task::JoinSet::new();
	loop {
		tokio::select! {
			_ = terminal.done.notified() => break,
			Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
			accepted = listener.accept() => {
				let (stream, _) = accepted?;
				if stream.peer_cred()?.uid() != rustix::process::geteuid().as_raw() || tasks.len() >= 16 { continue; }
				let terminal = Arc::clone(&terminal);
				tasks.spawn(async move {
					let result = timeout(TIMEOUT, connection(stream, Arc::clone(&terminal))).await;
					// Lost close acknowledgements cannot leave a stopped helper resident.
					if terminal.closing.load(std::sync::atomic::Ordering::SeqCst) && (result.is_err() || terminal.output_done.load(std::sync::atomic::Ordering::SeqCst)) { terminal.done.notify_one(); }
				});
			}
		}
	}
	std::fs::remove_file(socket)?;
	let retained = directory.canonicalize()?.join("jetfueld");
	if std::env::current_exe()?.canonicalize()? == retained {
		std::fs::remove_file(retained)?;
	}
	// Keep non-secret identity and bounded final replay after clean shell exit.
	Ok(())
}
fn pump(mut reader: Box<dyn Read + Send>, terminal: &Terminal) {
	let mut buffer = [0; 65536];
	let mut clean = true;
	loop {
		match reader.read(&mut buffer) {
			Ok(0) => break,
			Ok(count) => {
				if terminal
					.spool
					.lock()
					.expect("terminal spool")
					.append(&buffer[..count])
					.is_err()
				{
					clean = false;
					eprintln!("jetfueld: terminal output retention failed");
					let _ = terminal.pty.lock().expect("terminal PTY").close();
					break;
				}
			}
			Err(error) if error.kind() == io::ErrorKind::Interrupted => {
				continue;
			}
			Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
				if terminal
					.pty
					.lock()
					.expect("terminal PTY")
					.exited()
					.unwrap_or(false)
				{
					break;
				}
				std::thread::sleep(Duration::from_millis(10));
			}
			// Linux reports PTY EOF as EIO after the final slave closes.
			Err(error)
				if error.raw_os_error()
					== Some(rustix::io::Errno::IO.raw_os_error()) =>
			{
				break;
			}
			Err(_) => {
				clean = false;
				let _ = terminal.pty.lock().expect("terminal PTY").close();
				break;
			}
		}
	}
	loop {
		match terminal.pty.lock().expect("terminal PTY").exited() {
			Ok(true) => break,
			Ok(false) => std::thread::sleep(Duration::from_millis(10)),
			Err(_) => {
				eprintln!("jetfueld: terminal exit could not be established");
				return;
			}
		}
	}
	if clean
		&& terminal
			.spool
			.lock()
			.expect("terminal spool")
			.finish()
			.is_err()
	{
		eprintln!("jetfueld: terminal final replay could not be recorded");
	}
	terminal
		.output_done
		.store(true, std::sync::atomic::Ordering::SeqCst);
	if !terminal.closing.load(std::sync::atomic::Ordering::SeqCst) {
		terminal.done.notify_one();
	}
}
async fn connection(
	stream: UnixStream,
	terminal: Arc<Terminal>,
) -> io::Result<()> {
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	writer
		.write(&Frame::control(
			encode_control(&terminal.descriptor).map_err(io::Error::other)?,
		))
		.await
		.map_err(io::Error::other)?;
	let Frame::Control { payload, .. } =
		reader.read().await.map_err(io::Error::other)?
	else {
		return Err(invalid());
	};
	let request: TerminalHelperRequest =
		decode_control(&payload).map_err(io::Error::other)?;
	if request.instance != terminal.descriptor.instance
		|| request.protocol != terminal.descriptor.protocol
	{
		return Err(invalid());
	}
	reader.enable_multiplexing();
	writer.enable_multiplexing();
	let input = if matches!(request.action, TerminalHelperAction::Input) {
		let Frame::Data { stream_id, payload } =
			reader.read().await.map_err(io::Error::other)?
		else {
			return Err(invalid());
		};
		if stream_id.get() != 1 || payload.is_empty() || payload.len() > 65536 {
			return Err(invalid());
		}
		payload
	} else {
		vec![]
	};
	let worker = Arc::clone(&terminal);
	let close = matches!(request.action, TerminalHelperAction::Close);
	let replay = tokio::task::spawn_blocking(move || {
		match request.action {
			TerminalHelperAction::Read { after, limit } => {
				return worker
					.spool
					.lock()
					.expect("terminal spool")
					.read(after, limit);
			}
			TerminalHelperAction::Input => {
				if worker.pty.lock().expect("terminal PTY").exited()? {
					return Err(invalid());
				}
				worker
					.input
					.lock()
					.expect("terminal input")
					.write_all(&input)?;
			}
			TerminalHelperAction::Resize { rows, columns } => worker
				.pty
				.lock()
				.expect("terminal PTY")
				.resize(rows, columns)?,
			TerminalHelperAction::Close => {
				worker
					.closing
					.store(true, std::sync::atomic::Ordering::SeqCst);
				worker.pty.lock().expect("terminal PTY").close()?;
			}
		}
		worker.spool.lock().expect("terminal spool").read(0, 0)
	})
	.await
	.map_err(io::Error::other)??;
	let replay = if close {
		while !terminal
			.output_done
			.load(std::sync::atomic::Ordering::SeqCst)
		{
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
		terminal.spool.lock().expect("terminal spool").read(0, 0)?
	} else {
		replay
	};
	let reply = TerminalHelperReply {
		offset: replay.offset,
		produced: replay.produced,
		length: replay.bytes.len() as u32,
		closed: replay.closed || close,
	};
	let bytes = replay.bytes;
	writer
		.write(&Frame::control(
			encode_control(&reply).map_err(io::Error::other)?,
		))
		.await
		.map_err(io::Error::other)?;
	if !bytes.is_empty() {
		writer
			.write(&Frame::data(
				StreamId::new(1).expect("terminal stream"),
				bytes,
			))
			.await
			.map_err(io::Error::other)?;
	}
	if close {
		while !terminal
			.output_done
			.load(std::sync::atomic::Ordering::SeqCst)
		{
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
		terminal.done.notify_one();
	}
	Ok(())
}
fn invalid() -> io::Error {
	io::Error::other("invalid terminal helper request")
}
