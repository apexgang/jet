//! Craft handshakes and helper launch preparation.

use super::{CraftProcesses, RunConnection, TIMEOUT, filesystem};
use crate::run::craft::Contract;
use jet_core::{CoreError, LaunchPlan, RunId};
use jet_protocol::{
	CraftCommand, CraftHello, CraftHostAccess, CraftReady, Frame, FrameReader,
	FrameWriter, HelperConfig, Negotiation, ProtocolFamily, ProtocolOffer,
	ProtocolVersion, decode_control, encode_control,
};
use std::{
	os::unix::fs::{DirBuilderExt, OpenOptionsExt},
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
	process::Command,
	sync::Mutex,
	time::timeout,
};

pub(crate) async fn start(
	processes: &CraftProcesses,
	home: PathBuf,
	run_id: RunId,
	plan: &LaunchPlan,
) -> Result<(RunConnection, CraftCommand), CoreError> {
	plan.revalidate().await?;
	crate::run::recovery::validate_boot(plan)
		.await
		.map_err(|_| {
			failed("execution was accepted under a different OS boot")
		})?;
	let runtime = home.join("runtime");
	private_directory(runtime.clone()).await?;
	let (reader, writer) = craft_connection(
		processes,
		&runtime,
		run_id,
		plan,
		ConnectionMode::Launch,
	)
	.await?;
	let (socket, helper_pid) = helper(&runtime, run_id, plan).await?;
	let connection = RunConnection {
		limits_subagents: Contract::of(&plan.craft)?.limits_subagents(),
		child_work: Mutex::new(None),
		broker: crate::remote::no_visa_broker::Broker::prepare(
			processes, plan, run_id,
		)?,
		craft_minor: Contract::of(&plan.craft)?.craft_protocol.minor,
		run_id,
		reader: Mutex::new(reader),
		writer: Mutex::new(writer),
		helper_pid,
	};
	Ok((
		connection,
		CraftCommand::Start {
			id: plan.turn_id.unwrap_or(run_id.0).to_string(),
			text: plan.initial_input()?,
			helper_socket: socket.to_string_lossy().into_owned(),
		},
	))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionMode {
	Launch,
	Recovery,
}

pub(crate) async fn craft_connection(
	processes: &CraftProcesses,
	runtime: &Path,
	run_id: RunId,
	plan: &LaunchPlan,
	mode: ConnectionMode,
) -> Result<(FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>), CoreError>
{
	let broker = crate::remote::no_visa_broker::Broker::prepare(
		processes, plan, run_id,
	)?;
	let contract = Contract::of(&plan.craft)?;
	let mut stream = processes.connect(runtime, &plan.craft).await?;
	stream.write_all(b"jet-craft\n").await.map_err(failed)?;
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	let fork = match mode {
		ConnectionMode::Launch => plan.fork.as_ref().and_then(|fork| {
			fork.source_native_conversation.as_ref().map(|identity| {
				jet_protocol::CraftFork {
					source_native_conversation: identity.clone(),
					source_conversation_id: fork.source_conversation_id.0,
					source_run_id: fork.source_run_id.0,
					checkpoint_turn: fork.checkpoint_turn,
					checkpoint_commit: fork.checkpoint_commit.clone(),
					checkpoint_tree: fork.checkpoint_tree.clone(),
				}
			})
		}),
		ConnectionMode::Recovery => None,
	};
	let offer = ProtocolOffer {
		family: ProtocolFamily::Craft,
		versions: vec![contract.craft_protocol],
		capabilities: if plan.native_conversation.is_some() {
			vec!["runs".into(), "resume".into()]
		} else if fork.is_some() {
			vec!["fork".into(), "runs".into()]
		} else {
			vec!["runs".into()]
		},
	};
	let hello = CraftHello {
		protocol: offer.clone(),
		specification: ProtocolOffer {
			family: ProtocolFamily::Specification,
			versions: vec![ProtocolVersion { major: 1, minor: 0 }],
			capabilities: vec![],
		},
		execution_id: run_id.0,
		resume: plan.native_conversation.as_ref().map(|identity| {
			jet_protocol::CraftResume {
				version: contract.craft_protocol,
				native_conversation: identity.clone(),
				model: plan.model.as_ref().map(|model| model.0.clone()),
			}
		}),
		fork,
	};
	send(&mut writer, &hello).await?;
	let ready: CraftReady = timeout(TIMEOUT, receive(&mut reader))
		.await
		.map_err(failed)??;
	let expected = offer
		.negotiate(
			&contract.specification.protocol,
			hello
				.resume
				.as_ref()
				.map_or(Negotiation::NewExecution, |resume| {
					Negotiation::Resume(resume.version)
				}),
		)
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
	if let Some(broker) = broker {
		send(
			&mut writer,
			&CraftCommand::ConfigureRemoteTools {
				selection: selection_for_craft(&broker),
			},
		)
		.await?;
	}
	Ok((reader, writer))
}

pub(super) async fn helper(
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

pub(super) async fn private_directory(path: PathBuf) -> Result<(), CoreError> {
	filesystem::blocking(move || {
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(path)
	})
	.await?
	.map_err(failed)
}

pub(crate) async fn connect(path: &Path) -> Result<UnixStream, CoreError> {
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

pub(super) fn selection_for_craft(
	broker: &crate::remote::no_visa_broker::Broker,
) -> jet_protocol::NoVisaSelection {
	broker.selection()
}
