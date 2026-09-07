//! The helper alone owns the Harness, its pipes, and its terminal OS status.
use crate::spool::Spool;
use jet_protocol::{
	HelperConfig, HelperEvent, NativeInputMode, NativeSignal, NativeStream,
};
use std::{process::Stdio, sync::Arc};
use tokio::{
	io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
	process::Command,
};

/// A rejection is safe to settle only before spawn or after confirmed cleanup.
pub(crate) enum LaunchError {
	NotStarted,
	Unknown,
}
impl From<std::io::Error> for LaunchError {
	fn from(_: std::io::Error) -> Self {
		Self::NotStarted
	}
}

pub(crate) struct Control {
	pub(crate) stop: tokio::sync::watch::Receiver<bool>,
	pub(crate) stopped: tokio::sync::watch::Sender<bool>,
	/// Escalation steps requested by the host, each with the sequence that
	/// distinguishes a repeat of the same signal from the previous one.
	pub(crate) signal:
		tokio::sync::watch::Receiver<Option<(u64, NativeSignal)>>,
}

/// Open standard input of a `Streaming` Harness. Bytes stay opaque: the
/// helper writes them verbatim and frames nothing. Awaiting a write applies
/// the Harness's own backpressure to the Craft that asked for it. A clone
/// keeps the input open only for as long as that clone lives.
#[derive(Clone)]
pub(crate) struct NativeInput {
	writes: tokio::sync::mpsc::Sender<String>,
}

impl NativeInput {
	/// Write native input, waiting while the Harness has not drained earlier
	/// bytes. Fails once the Harness or its input is gone.
	pub(crate) async fn write(&self, text: String) -> std::io::Result<()> {
		self.writes
			.send(text)
			.await
			.map_err(|_| std::io::Error::other("native input is closed"))
	}
}

pub(crate) async fn launch(
	config: &HelperConfig,
	program: String,
	arguments: Vec<String>,
	input: String,
	input_mode: NativeInputMode,
	spool: Arc<Spool>,
	mut control: Control,
) -> Result<Option<NativeInput>, LaunchError> {
	if !config.executables.contains(&program)
		|| arguments.len() > 256
		|| arguments.iter().map(String::len).sum::<usize>() > 65_536
		|| input.len() > 65_536
	{
		return Err(LaunchError::NotStarted);
	}
	let root = std::path::Path::new(&config.working_directory);
	if root.canonicalize()? != root {
		return Err(LaunchError::NotStarted);
	}
	// ASVS 1.2.5: no shell interpretation of the accepted executable, arguments,
	// or input. Its working root comes exclusively from the authoritative host.
	let mut child = Command::new(program)
		.args(arguments)
		.current_dir(root)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.process_group(0)
		.kill_on_drop(false)
		.spawn()?;
	let harness_pid = child
		.id()
		.expect("freshly spawned process has an OS identity");
	if let Err(error) = spool.append(HelperEvent::Started { harness_pid }).await
	{
		// Do not report a definite failure until the spawned child was stopped.
		child.kill().await.map_err(|_| LaunchError::Unknown)?;
		return Err(error.into());
	}
	let mut stdin = child.stdin.take().expect("piped input");
	let stdout = child.stdout.take().expect("piped output");
	let stderr = child.stderr.take().expect("piped errors");
	// A sealed launch drops its sender at once, so the writer sees end of
	// input immediately after the initial write and the Harness reads EOF.
	let (writes, mut pending) = tokio::sync::mpsc::channel::<String>(8);
	let native_input = match input_mode {
		NativeInputMode::Sealed => None,
		NativeInputMode::Streaming => Some(NativeInput { writes }),
	};
	tokio::spawn(async move {
		let mut input_task = tokio::spawn(async move {
			stdin.write_all(input.as_bytes()).await?;
			stdin.flush().await?;
			while let Some(text) = pending.recv().await {
				stdin.write_all(text.as_bytes()).await?;
				stdin.flush().await?;
			}
			// Dropping the pipe here is the close, and the only one.
			Ok::<(), std::io::Error>(())
		});
		let mut output_task = tokio::spawn(pump(
			stdout,
			NativeStream::Stdout,
			Arc::clone(&spool),
		));
		let mut error_task = tokio::spawn(pump(
			stderr,
			NativeStream::Stderr,
			Arc::clone(&spool),
		));
		let completed = async {
			// `wait` is cancel safe: a branch that wins the race leaves the
			// child unreaped, so signalling its group stays well defined.
			let status = loop {
				tokio::select! {
					status = child.wait() => break status,
					Ok(()) = control.signal.changed() => {
						if let Some((_, signal)) =
							*control.signal.borrow_and_update()
						{
							let _ = deliver(harness_pid, signal);
						}
					}
				}
			};
			// The child is reaped, so nothing can consume further input and a
			// streaming writer would otherwise wait for a close that no
			// longer means anything.
			input_task.abort();
			let _ = (&mut input_task).await;
			let output = (&mut output_task).await;
			let errors = (&mut error_task).await;
			if !matches!(output, Ok(Ok(()))) || !matches!(errors, Ok(Ok(()))) {
				return None;
			}
			Some(status)
		};
		let stop = async {
			if !*control.stop.borrow() {
				let _ = control.stop.changed().await;
			}
		};
		let status = tokio::select! {
			status = completed => status,
			_ = stop => {
				// The helper signals only its unreaped child, in the process group
				// it created. A descriptor-supplied PID can never choose this target.
				if matches!(child.try_wait(), Ok(None))
					&& deliver(harness_pid, NativeSignal::Kill).is_err() { return; }
				if child.wait().await.is_err() { return; }
				input_task.abort(); output_task.abort(); error_task.abort();
				control.stopped.send_replace(true);
				return;
			}
		};
		let Some(status) = status else {
			eprintln!("jetfueld: native output could not be retained");
			return;
		};
		control.stopped.send_replace(true);
		let exit_code = status.ok().and_then(|s| s.code());
		if spool
			.append(HelperEvent::Exited { exit_code })
			.await
			.is_err()
		{
			eprintln!("jetfueld: native exit could not be retained");
		}
	});
	Ok(native_input)
}

/// Delivers one signal to the process group the helper created for its
/// child. A host-supplied identity can never choose this target.
fn deliver(
	harness_pid: u32,
	signal: NativeSignal,
) -> Result<(), rustix::io::Errno> {
	let pid =
		rustix::process::Pid::from_raw(harness_pid as i32).expect("native PID");
	rustix::process::kill_process_group(
		pid,
		match signal {
			NativeSignal::Interrupt => rustix::process::Signal::INT,
			NativeSignal::Terminate => rustix::process::Signal::TERM,
			NativeSignal::Kill => rustix::process::Signal::KILL,
		},
	)
}

async fn pump(
	mut pipe: impl AsyncRead + Unpin,
	stream: NativeStream,
	spool: Arc<Spool>,
) -> std::io::Result<()> {
	let mut buffer = [0; 4096];
	loop {
		let count = pipe.read(&mut buffer).await?;
		if count == 0 {
			return Ok(());
		}
		spool
			.append(HelperEvent::Output {
				stream,
				bytes: buffer[..count].to_vec(),
			})
			.await?;
	}
}
