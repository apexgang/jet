//! The helper alone owns the Harness, its pipes, and its terminal OS status.
use crate::spool::Spool;
use jet_protocol::{HelperConfig, HelperEvent, NativeStream};
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
}

pub(crate) async fn launch(
	config: &HelperConfig,
	program: String,
	arguments: Vec<String>,
	input: String,
	spool: Arc<Spool>,
	mut control: Control,
) -> Result<(), LaunchError> {
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
	tokio::spawn(async move {
		let mut input_task =
			tokio::spawn(
				async move { stdin.write_all(input.as_bytes()).await },
			);
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
			let status = child.wait().await;
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
				if matches!(child.try_wait(), Ok(None)) {
					let pid = rustix::process::Pid::from_raw(harness_pid as i32).expect("native PID");
					if rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).is_err() { return; }
				}
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
	Ok(())
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
