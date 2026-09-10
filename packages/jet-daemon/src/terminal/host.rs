//! Instance-bound terminal helpers, pinned to durable Workspace authority.
use jet_core::{
	CoreError, RunFuture, TerminalHost, TerminalOperation, TerminalOutput,
	TerminalPlan,
};
use jet_protocol::*;
use std::{
	io,
	os::unix::fs::{DirBuilderExt, OpenOptionsExt},
	path::{Path, PathBuf},
	process::Stdio,
};
use tokio::{
	net::UnixStream,
	process::Command,
	time::{Duration, timeout},
};

const TIMEOUT: Duration = Duration::from_secs(10);
#[derive(Debug)]
pub(crate) struct Terminals;
impl TerminalHost for Terminals {
	fn discover(
		&self,
		home: PathBuf,
	) -> RunFuture<'_, Result<Vec<jet_core::TerminalId>, CoreError>> {
		Box::pin(crate::terminal::recovery::discover(home))
	}
	fn inspect(
		&self,
		home: PathBuf,
		id: jet_core::TerminalId,
	) -> RunFuture<'_, Result<Option<jet_core::ExecutionMetadata>, CoreError>>
	{
		Box::pin(crate::terminal::recovery::inspect(home, id))
	}
	fn terminate(
		&self,
		home: PathBuf,
		request: jet_core::ExecutionResolution,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(crate::terminal::recovery::terminate(home, request))
	}
	fn start(
		&self,
		home: PathBuf,
		plan: TerminalPlan,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			timeout(TIMEOUT, start(home, plan)).await.map_err(failed)?
		})
	}
	fn operate(
		&self,
		home: PathBuf,
		plan: TerminalPlan,
		operation: TerminalOperation,
	) -> RunFuture<'_, Result<TerminalOutput, CoreError>> {
		Box::pin(async move {
			let input = matches!(operation, TerminalOperation::Input(_));
			let result = timeout(TIMEOUT, operate(home, plan, operation))
				.await
				.map_err(failed)
				.and_then(|r| r);
			result.map_err(|mut error| {
				if input { error.category = jet_core::ErrorCategory::OutcomeUnknown; error.code = "terminal.input_outcome_unknown".into(); error.message = "terminal input may have been delivered; do not replay it automatically".into(); }
				error
			})
		})
	}
}
fn config(plan: &TerminalPlan) -> TerminalConfig {
	TerminalConfig {
		terminal_id: plan.terminal_id.0,
		workspace_id: plan.workspace_id.0,
		root: plan.root.to_string_lossy().into_owned(),
		project_root: plan.project_root.to_string_lossy().into_owned(),
		boot: plan.boot.clone(),
		shell: "/bin/sh".into(),
		rows: plan.rows,
		columns: plan.columns,
	}
}
fn directory(home: &Path, plan: &TerminalPlan) -> PathBuf {
	home.join("runtime")
		.join(format!("t-{}", plan.terminal_id.0.simple()))
}
async fn start(home: PathBuf, plan: TerminalPlan) -> Result<(), CoreError> {
	let directory = directory(&home, &plan);
	let setup = directory.clone();
	let boundary = config(&plan);
	let digest = plan.helper_digest.clone();
	tokio::task::spawn_blocking(move || {
		if crate::terminal::recovery::boot()? != boundary.boot
			|| Path::new(&boundary.root).canonicalize()?
				!= Path::new(&boundary.root)
		{
			return Err(io::Error::other("terminal launch boundary changed"));
		}
		let runtime = setup.parent().expect("execution parent");
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(runtime)?;
		jet_runtime::validate_execution_directory(runtime)?;
		// ADR-0067: create_new fences uncertain attempts; no restart path calls spawn.
		std::fs::DirBuilder::new().mode(0o700).create(&setup)?;
		let mut file = std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(setup.join("config.json"))?;
		std::io::Write::write_all(
			&mut file,
			&encode_control(&boundary).map_err(io::Error::other)?,
		)?;
		file.sync_all()?;
		let mut source = std::fs::File::open(
			std::env::current_exe()?.with_file_name("jetfueld"),
		)?;
		let mut retained = std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o700)
			.open(setup.join("jetfueld"))?;
		std::io::copy(&mut source, &mut retained)?;
		retained.sync_all()?;
		if jet_runtime::execution_digest(&setup.join("jetfueld"))? != digest {
			return Err(io::Error::other("helper changed"));
		}
		std::fs::File::open(setup)?.sync_all()
	})
	.await
	.map_err(failed)?
	.map_err(failed)?;
	let mut child = Command::new(directory.join("jetfueld"))
		.arg("terminal")
		.arg("--config")
		.arg(directory.join("config.json"))
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.process_group(0)
		.kill_on_drop(false)
		.spawn()
		.map_err(failed)?;
	loop {
		if let Ok(output) = operate(
			home.clone(),
			plan.clone(),
			TerminalOperation::Read { after: 0, limit: 0 },
		)
		.await && !output.closed
		{
			return Ok(());
		}
		if child.try_wait().map_err(failed)?.is_some() {
			return Err(failed("terminal helper exited during startup"));
		}
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
}
async fn operate(
	home: PathBuf,
	plan: TerminalPlan,
	operation: TerminalOperation,
) -> Result<TerminalOutput, CoreError> {
	let directory = directory(&home, &plan);
	let path = directory.clone();
	let accepted = plan.clone();
	let (descriptor, retained) = tokio::task::spawn_blocking(move || {
		if crate::terminal::recovery::boot()? != accepted.boot {
			return Ok((None, false));
		}
		let bytes =
			jet_runtime::read_execution_file(&path.join("descriptor.json"))?;
		let descriptor: TerminalDescriptor =
			decode_control(&bytes).map_err(io::Error::other)?;
		if descriptor.protocol != (ProtocolVersion { major: 1, minor: 0 })
			|| descriptor.role != "terminal"
			|| descriptor.config != config(&accepted)
			|| descriptor.sha256 != accepted.helper_digest
		{
			return Err(io::Error::other("terminal descriptor mismatch"));
		}
		match jet_runtime::execution_process_identity(descriptor.pid)? {
			None => Ok((None, true)),
			Some(identity) if identity == descriptor.process_start => {
				Ok((Some(descriptor), false))
			}
			Some(_) => {
				Err(io::Error::other("terminal process identity changed"))
			}
		}
	})
	.await
	.map_err(failed)?
	.map_err(failed)?;
	let Some(expected) = descriptor else {
		return match operation {
			TerminalOperation::Read { after, limit } if retained => {
				let replay = tokio::task::spawn_blocking(move || {
					jet_runtime::TerminalSpool::read_retained(
						&directory, after, limit,
					)
				})
				.await
				.map_err(failed)?
				.map_err(failed)?;
				Ok(TerminalOutput {
					offset: replay.offset,
					produced: replay.produced,
					bytes: replay.bytes,
					closed: true,
				})
			}
			TerminalOperation::Read { after, .. } => Ok(TerminalOutput {
				offset: after,
				produced: after,
				bytes: vec![],
				closed: true,
			}),
			TerminalOperation::Close => Ok(TerminalOutput {
				offset: 0,
				produced: 0,
				bytes: vec![],
				closed: true,
			}),
			TerminalOperation::Input(_) | TerminalOperation::Resize { .. } => {
				Err(failed("terminal closed"))
			}
		};
	};
	exchange(directory, expected, operation).await
}
pub(crate) async fn exchange(
	directory: PathBuf,
	expected: TerminalDescriptor,
	operation: TerminalOperation,
) -> Result<TerminalOutput, CoreError> {
	let stream = UnixStream::connect(directory.join("h.sock"))
		.await
		.map_err(failed)?;
	let peer = stream.peer_cred().map_err(failed)?;
	if peer.uid() != rustix::process::geteuid().as_raw()
		|| peer.pid().is_some_and(|pid| pid as u32 != expected.pid)
	{
		return Err(failed("terminal peer mismatch"));
	}
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let descriptor: TerminalDescriptor =
		crate::run::host::receive(&mut reader).await?;
	if descriptor != expected {
		return Err(failed("terminal instance mismatch"));
	}
	let (action, input) = match operation {
		TerminalOperation::Read { after, limit } => {
			(TerminalHelperAction::Read { after, limit }, vec![])
		}
		TerminalOperation::Input(bytes) => (TerminalHelperAction::Input, bytes),
		TerminalOperation::Resize { rows, columns } => {
			(TerminalHelperAction::Resize { rows, columns }, vec![])
		}
		TerminalOperation::Close => (TerminalHelperAction::Close, vec![]),
	};
	crate::run::host::send(
		&mut writer,
		&TerminalHelperRequest {
			protocol: descriptor.protocol,
			instance: descriptor.instance,
			action,
		},
	)
	.await?;
	reader.enable_multiplexing();
	writer.enable_multiplexing();
	if !input.is_empty() {
		writer
			.write(&Frame::data(
				StreamId::new(1).expect("terminal stream"),
				input,
			))
			.await
			.map_err(failed)?;
	}
	let reply: TerminalHelperReply =
		crate::run::host::receive(&mut reader).await?;
	if reply.length > 65536
		|| reply.offset > reply.produced
		|| u64::from(reply.length) > reply.produced - reply.offset
	{
		return Err(failed("invalid terminal replay"));
	}
	let bytes = if reply.length == 0 {
		vec![]
	} else {
		let Frame::Data { stream_id, payload } =
			reader.read().await.map_err(failed)?
		else {
			return Err(failed("missing terminal bytes"));
		};
		if stream_id.get() != 1 || payload.len() != reply.length as usize {
			return Err(failed("invalid terminal bytes"));
		}
		payload
	};
	Ok(TerminalOutput {
		offset: reply.offset,
		produced: reply.produced,
		bytes,
		closed: reply.closed,
	})
}
pub(crate) fn failed(error: impl std::fmt::Display) -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: "terminal.transport_unavailable".into(),
		retryable: false,
		message: "the terminal helper connection is unavailable".into(),
		detail: Some(error.to_string()),
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
