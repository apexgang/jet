//! Concrete out-of-process Craft and helper connections, pinned by accepted digest.
use crate::run_craft::{self, Contract};
use jet_core::{
	CoreError, LaunchPlan, PinnedCraft, RunFuture, RunHost, RunId,
	RunObservation, RunStartError,
};
use jet_protocol::{
	CraftCommand, CraftEvent, CraftHello, CraftHostAccess, CraftReady, Frame,
	FrameReader, FrameWriter, HelperConfig, Negotiation, ProtocolFamily,
	ProtocolOffer, ProtocolVersion, decode_control, encode_control,
};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	process::Stdio,
	time::Duration,
};
use tokio::{
	io::AsyncWriteExt,
	net::{
		UnixStream,
		unix::{OwnedReadHalf, OwnedWriteHalf},
	},
	process::{Child, Command},
	sync::Mutex,
	time::timeout,
};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(10);
#[derive(Debug, Default)]
pub(crate) struct CraftProcesses(Mutex<HashMap<String, CraftProcess>>);
#[derive(Debug)]
struct CraftProcess {
	child: Child,
	socket: PathBuf,
}

pub(crate) struct RunConnection {
	pub(crate) reader: FrameReader<OwnedReadHalf>,
	pub(crate) writer: FrameWriter<OwnedWriteHalf>,
	pub(crate) helper_pid: u32,
	run_id: RunId,
}
impl jet_core::RunConnection for RunConnection {
	fn receive(&mut self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
		Box::pin(async move {
			let event: CraftEvent = receive(&mut self.reader).await?;
			Ok(match event {
				CraftEvent::RunStarted {
					helper_pid,
					harness_pid,
				} => {
					if helper_pid != self.helper_pid {
						return Err(failed("wrong helper identity"));
					}
					RunObservation::Started {
						helper_pid,
						harness_pid,
					}
				}
				CraftEvent::RunLaunchFailed => RunObservation::LaunchFailed,
				CraftEvent::Activity { activity } => {
					RunObservation::Activity(activity_from_wire(activity))
				}
				CraftEvent::Output {
					native_event,
					presentation,
				} => RunObservation::Output {
					native_json: native_event.get().into(),
					presentation_json: presentation
						.into_iter()
						.map(|p| p.raw().get().to_owned())
						.collect(),
				},
				CraftEvent::Completed {
					id,
					native_conversation,
				} => {
					if id != self.run_id.0.to_string() {
						return Err(failed("wrong completion identity"));
					}
					RunObservation::NativeConversation(native_conversation)
				}
				CraftEvent::RunEnded { exit_code } => {
					RunObservation::Ended(exit_code)
				}
				CraftEvent::Progress { source_offset } => {
					RunObservation::Progress(source_offset)
				}
			})
		})
	}
	fn acknowledge(
		&mut self,
		source_offset: u64,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			send(
				&mut self.writer,
				&CraftCommand::Acknowledge { source_offset },
			)
			.await
		})
	}
	fn finish(&mut self) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			send(&mut self.writer, &CraftCommand::Shutdown).await
		})
	}
}
impl RunHost for CraftProcesses {
	fn pin(
		&self,
		home: PathBuf,
		id: String,
	) -> RunFuture<'_, Result<PinnedCraft, CoreError>> {
		Box::pin(async move { run_craft::load(&home, &id).await })
	}
	fn start(
		&self,
		home: PathBuf,
		run_id: RunId,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn jet_core::RunConnection>, RunStartError>>
	{
		Box::pin(async move {
			let (mut connection, command) = start(self, home, run_id, &plan)
				.await
				.map_err(|_| RunStartError::NotStarted)?;
			send(&mut connection.writer, &command)
				.await
				.map_err(|_| RunStartError::Unknown)?;
			Ok(Box::new(connection) as Box<dyn jet_core::RunConnection>)
		})
	}
}
pub(crate) async fn start(
	processes: &CraftProcesses,
	home: PathBuf,
	run_id: RunId,
	plan: &LaunchPlan,
) -> Result<(RunConnection, CraftCommand), CoreError> {
	plan.revalidate().await?;
	let contract = Contract::of(&plan.craft)?;
	let runtime = home.join("runtime");
	private_directory(runtime.clone()).await?;
	let mut stream = processes.connect(&runtime, &plan.craft).await?;
	stream.write_all(b"jet-craft\n").await.map_err(failed)?;
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let offer = ProtocolOffer {
		family: ProtocolFamily::Craft,
		versions: vec![ProtocolVersion { major: 1, minor: 1 }],
		capabilities: vec!["runs".into()],
	};
	let hello = CraftHello {
		protocol: offer.clone(),
		specification: ProtocolOffer {
			family: ProtocolFamily::Specification,
			versions: vec![ProtocolVersion { major: 1, minor: 0 }],
			capabilities: vec![],
		},
		execution_id: run_id.0,
		resume: None,
	};
	send(&mut writer, &hello).await?;
	let ready: CraftReady = timeout(TIMEOUT, receive(&mut reader))
		.await
		.map_err(failed)??;
	let expected = offer
		.negotiate(&contract.specification.protocol, Negotiation::NewExecution)
		.map_err(failed)?;
	if ready.specification != contract.specification
		|| ready.protocol != expected
		|| ready.specification_protocol
			!= hello
				.specification
				.negotiate(&hello.specification, Negotiation::NewExecution)
				.map_err(failed)?
		|| ready.enabled_features
			!= contract.specification.enabled_features().map_err(failed)?
	{
		return Err(failed("Craft declarations changed"));
	}
	reader.enable_multiplexing();
	writer.enable_multiplexing();
	let (socket, helper_pid) = helper(&runtime, run_id, plan).await?;
	let connection = RunConnection {
		run_id,
		reader,
		writer,
		helper_pid,
	};
	Ok((
		connection,
		CraftCommand::Start {
			id: run_id.0.to_string(),
			text: plan.prompt.clone(),
			helper_socket: socket.to_string_lossy().into_owned(),
		},
	))
}

impl CraftProcesses {
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the async gate spans process startup so concurrent Runs cannot spawn duplicate Crafts for one digest (ADR-0018)"
	)]
	async fn connect(
		&self,
		runtime: &Path,
		pin: &PinnedCraft,
	) -> Result<UnixStream, CoreError> {
		let mut processes = self.0.lock().await;
		if let Some(process) = processes.get_mut(&pin.sha256)
			&& process.child.try_wait().map_err(failed)?.is_none()
		{
			return connect(&process.socket).await;
		}
		let socket =
			runtime.join(format!("c-{}.sock", Uuid::new_v4().simple()));
		pin.verify().await?;
		// ASVS 1.2.5: the installed executable receives a private endpoint,
		// never a client-supplied command line. One process multiplexes its Runs.
		let child = Command::new(&pin.executable)
			.arg("--socket")
			.arg(&socket)
			.stdin(Stdio::null())
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.kill_on_drop(true)
			.spawn()
			.map_err(failed)?;
		let stream = connect(&socket).await?;
		processes.insert(pin.sha256.clone(), CraftProcess { child, socket });
		Ok(stream)
	}
}

async fn helper(
	runtime: &Path,
	run_id: RunId,
	plan: &LaunchPlan,
) -> Result<(PathBuf, u32), CoreError> {
	let directory = runtime.join(run_id.0.simple().to_string());
	let config = HelperConfig {
		execution_id: run_id.0,
		working_directory: plan.root.to_string_lossy().into_owned(),
		executables: Contract::of(&plan.craft)?
			.specification
			.host_access
			.iter()
			.filter_map(|access| match access {
				CraftHostAccess::Executable { name } => Some(name.clone()),
				CraftHostAccess::Filesystem { .. }
				| CraftHostAccess::Environment { .. }
				| CraftHostAccess::Network { .. } => None,
			})
			.collect(),
	};
	let config_path = directory.join("config.json");
	let path = config_path.clone();
	filesystem::blocking(move || {
		// create_new is the stable external identity barrier. Uncertain prior
		// attempts are reconciled, never overwritten or automatically relaunched.
		std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
		let mut file = std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o600)
			.open(path)?;
		std::io::Write::write_all(
			&mut file,
			&encode_control(&config).map_err(std::io::Error::other)?,
		)?;
		file.sync_all()
	})
	.await?
	.map_err(failed)?;
	let executable = std::env::current_exe()
		.map_err(failed)?
		.with_file_name("jetfueld");
	let child = Command::new(executable)
		.arg("run")
		.arg("--config")
		.arg(&config_path)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.kill_on_drop(false)
		.spawn()
		.map_err(failed)?;
	let pid = child
		.id()
		.ok_or_else(|| failed("helper identity unavailable"))?;
	// Dropping this handle deliberately leaves the helper owning its Harness.
	let socket = config_path.with_file_name("h.sock");
	drop(connect(&socket).await?);
	Ok((socket, pid))
}

async fn private_directory(path: PathBuf) -> Result<(), CoreError> {
	filesystem::blocking(move || {
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(path)
	})
	.await?
	.map_err(failed)
}
async fn connect(path: &Path) -> Result<UnixStream, CoreError> {
	timeout(TIMEOUT, async {
		loop {
			match UnixStream::connect(path).await {
				Ok(stream) => return Ok(stream),
				Err(error)
					if matches!(
						error.kind(),
						std::io::ErrorKind::NotFound
							| std::io::ErrorKind::ConnectionRefused
					) =>
				{
					tokio::time::sleep(Duration::from_millis(10)).await
				}
				Err(error) => return Err(failed(error)),
			}
		}
	})
	.await
	.map_err(failed)?
}
async fn receive<T: serde::de::DeserializeOwned>(
	reader: &mut FrameReader<OwnedReadHalf>,
) -> Result<T, CoreError> {
	match reader.read().await.map_err(failed)? {
		Frame::Control { stream_id, payload } if stream_id.is_connection() => {
			decode_control(&payload).map_err(failed)
		}
		Frame::Control { .. } | Frame::Data { .. } => {
			Err(failed("expected Craft control"))
		}
	}
}
async fn send(
	writer: &mut FrameWriter<OwnedWriteHalf>,
	value: &impl serde::Serialize,
) -> Result<(), CoreError> {
	let payload = encode_control(value).map_err(failed)?;
	timeout(TIMEOUT, writer.write(&Frame::control(payload)))
		.await
		.map_err(failed)?
		.map_err(failed)
}
pub(crate) fn failed(error: impl std::fmt::Display) -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: "run.transport_unavailable".into(),
		retryable: true,
		message: "the Run execution connection is unavailable".into(),
		detail: Some(error.to_string()),
		revision_conflict: None,
		recovery_actions: vec![],
	}
}

fn activity_from_wire(
	value: jet_protocol::RunActivity,
) -> jet_core::RunActivity {
	match value {
		jet_protocol::RunActivity::Working => jet_core::RunActivity::Working,
		jet_protocol::RunActivity::WaitingForUser => {
			jet_core::RunActivity::WaitingForUser
		}
		jet_protocol::RunActivity::WaitingForApproval => {
			jet_core::RunActivity::WaitingForApproval
		}
		jet_protocol::RunActivity::WaitingForAuth => {
			jet_core::RunActivity::WaitingForAuth
		}
		jet_protocol::RunActivity::WaitingForQuota => {
			jet_core::RunActivity::WaitingForQuota
		}
		jet_protocol::RunActivity::Reconnecting => {
			jet_core::RunActivity::Reconnecting
		}
	}
}

// File operations run off the daemon's async connection workers.
pub(crate) mod filesystem {
	use jet_core::CoreError;
	pub(crate) async fn blocking<T: Send + 'static>(
		work: impl FnOnce() -> T + Send + 'static,
	) -> Result<T, CoreError> {
		tokio::task::spawn_blocking(work)
			.await
			.map_err(super::failed)
	}
}
