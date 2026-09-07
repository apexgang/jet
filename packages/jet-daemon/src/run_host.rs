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
	pub(crate) craft_minor: u32,
	pub(crate) reader: FrameReader<OwnedReadHalf>,
	pub(crate) writer: FrameWriter<OwnedWriteHalf>,
	pub(crate) helper_pid: u32,
	pub(crate) run_id: RunId,
}
impl jet_core::RunConnection for RunConnection {
	fn receive(&mut self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
		Box::pin(async move {
			let event: CraftEvent = receive(&mut self.reader).await?;
			if self.craft_minor < 3
				&& matches!(
					&event,
					CraftEvent::TurnStarted
						| CraftEvent::TurnEnded { .. }
						| CraftEvent::FileChanged { .. }
				) {
				return Err(failed("change evidence requires Craft 1.3"));
			}
			Ok(match event {
				CraftEvent::TurnStarted => RunObservation::TurnStarted,
				CraftEvent::TurnEnded { outcome } => {
					RunObservation::TurnEnded(match outcome {
						jet_protocol::TurnOutcome::Completed => {
							jet_core::TurnOutcome::Completed
						}
						jet_protocol::TurnOutcome::Interrupted => {
							jet_core::TurnOutcome::Interrupted
						}
					})
				}
				CraftEvent::FileChanged { change } => {
					RunObservation::FileChanged(jet_core::ChangeEvidence {
						activity_id: change.activity_id,
						path: change.path,
						before_object: change.before_object,
						after_object: change.after_object,
						before_mode: change.before_mode,
						after_mode: change.after_mode,
						origin: jet_core::ChangeOrigin::Harness {
							run_id: self.run_id,
						},
					})
				}
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
				CraftEvent::RunRecovered { .. } => {
					return Err(failed("unexpected recovery handshake"));
				}
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
					RunObservation::Completed(native_conversation)
				}
				CraftEvent::RunEnded { exit_code } => {
					RunObservation::Ended(exit_code)
				}
				CraftEvent::Progress {
					source_offset,
					checkpoint,
				} => RunObservation::Progress {
					offset: source_offset,
					checkpoint,
				},
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
	fn recover(
		&self,
		home: PathBuf,
		run_id: RunId,
		plan: LaunchPlan,
		cursor: jet_core::RunRecoveryCursor,
	) -> RunFuture<
		'_,
		Result<Box<dyn jet_core::RunConnection>, jet_core::RunRecoveryError>,
	> {
		Box::pin(crate::run_recovery::connect(
			self, home, run_id, plan, cursor,
		))
	}
	fn discover(
		&self,
		home: PathBuf,
	) -> RunFuture<'_, Result<Vec<RunId>, CoreError>> {
		Box::pin(crate::run_recovery::discover(home))
	}

	fn probe(
		&self,
		home: PathBuf,
		id: RunId,
		accepted: Option<LaunchPlan>,
		helper_pid: Option<u32>,
	) -> RunFuture<'_, Result<(), jet_core::RunRecoveryError>> {
		Box::pin(async move {
			if let Some(plan) = accepted {
				crate::run_recovery::validate_boot(&plan).await?;
			}
			crate::run_recovery::identity(home, id, helper_pid)
				.await
				.map(|_| ())
		})
	}
	fn describe(
		&self,
		home: PathBuf,
		id: RunId,
	) -> RunFuture<'_, Result<jet_core::ExecutionMetadata, CoreError>> {
		Box::pin(crate::run_recovery::describe(home, id))
	}
	fn validate_recovery(
		&self,
		home: PathBuf,
		id: RunId,
		plan: LaunchPlan,
		cursor: jet_core::RunRecoveryCursor,
	) -> RunFuture<'_, Result<(), jet_core::RunRecoveryError>> {
		Box::pin(async move {
			crate::run_recovery::validate(home, id, &plan, &cursor)
				.await
				.map(|_| ())
		})
	}

	fn terminate(
		&self,
		home: PathBuf,
		request: jet_core::ExecutionResolution,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(crate::execution_termination::terminate(home, request))
	}
}
pub(crate) async fn start(
	processes: &CraftProcesses,
	home: PathBuf,
	run_id: RunId,
	plan: &LaunchPlan,
) -> Result<(RunConnection, CraftCommand), CoreError> {
	plan.revalidate().await?;
	crate::run_recovery::validate_boot(plan)
		.await
		.map_err(|_| {
			failed("execution was accepted under a different OS boot")
		})?;
	let runtime = home.join("runtime");
	private_directory(runtime.clone()).await?;
	let (reader, writer) =
		craft_connection(processes, &runtime, run_id, plan).await?;
	let (socket, helper_pid) = helper(&runtime, run_id, plan).await?;
	let connection = RunConnection {
		craft_minor: Contract::of(&plan.craft)?.craft_protocol.minor,
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

pub(crate) async fn craft_connection(
	processes: &CraftProcesses,
	runtime: &Path,
	run_id: RunId,
	plan: &LaunchPlan,
) -> Result<(FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>), CoreError>
{
	let contract = Contract::of(&plan.craft)?;
	let mut stream = processes.connect(runtime, &plan.craft).await?;
	stream.write_all(b"jet-craft\n").await.map_err(failed)?;
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let offer = ProtocolOffer {
		family: ProtocolFamily::Craft,
		versions: vec![contract.craft_protocol],
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
	Ok((reader, writer))
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
	let config = helper_config(run_id, plan)?;
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
		file.sync_all()?;
		// ADR-0088: retain the accepted executable across installation updates.
		let mut source = std::fs::File::open(
			std::env::current_exe()?.with_file_name("jetfueld"),
		)?;
		let mut artifact = std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.mode(0o700)
			.open(directory.join("jetfueld"))?;
		std::io::copy(&mut source, &mut artifact)?;
		artifact.sync_all()
	})
	.await?
	.map_err(failed)?;
	let executable = config_path.with_file_name("jetfueld");
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

pub(crate) fn helper_config(
	run_id: RunId,
	plan: &LaunchPlan,
) -> Result<HelperConfig, CoreError> {
	Ok(HelperConfig {
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
		project_directory: plan.project_root.to_string_lossy().into_owned(),
		craft_digest: plan.craft.sha256.clone(),
	})
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
pub(crate) async fn receive<T: serde::de::DeserializeOwned>(
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
pub(crate) async fn send(
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
