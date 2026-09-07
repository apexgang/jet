//! Validate helper identity before reconnecting a pinned Craft to its source.
use crate::run_host::{self, CraftProcesses, RunConnection, filesystem};
use jet_core::{
	CoreError, LaunchPlan, RunId, RunRecoveryCursor, RunRecoveryError,
};
use jet_protocol::{
	HelperCommand, HelperDescriptor, HelperHello, HelperReady, ProtocolFamily,
	ProtocolOffer, ProtocolVersion,
};
use std::{path::PathBuf, time::Duration};
use tokio::{net::UnixStream, time::timeout};

pub(crate) async fn discover(home: PathBuf) -> Result<Vec<RunId>, CoreError> {
	filesystem::blocking(move || {
		let runtime = home.join("runtime");
		if !runtime.exists() {
			return Ok(vec![]);
		}
		jet_runtime::validate_execution_directory(&runtime)?;
		let mut ids = Vec::new();
		for entry in std::fs::read_dir(runtime)? {
			let entry = entry?;
			if let Some(name) = entry.file_name().to_str()
				&& let Ok(id) = uuid::Uuid::parse_str(name)
				&& entry
					.path()
					.join("descriptor.json")
					.symlink_metadata()
					.is_ok()
			{
				ids.push(RunId(id));
				if ids.len() > 4096 {
					return Err(std::io::Error::other(
						"too many runtime executions",
					));
				}
			}
		}
		Ok::<_, std::io::Error>(ids)
	})
	.await?
	.map_err(run_host::failed)
}

pub(crate) async fn describe(
	home: PathBuf,
	id: RunId,
) -> Result<jet_core::ExecutionMetadata, CoreError> {
	filesystem::blocking(move || {
		let bytes = jet_runtime::read_execution_file(
			&home
				.join("runtime")
				.join(id.0.simple().to_string())
				.join("descriptor.json"),
		)?;
		let descriptor: HelperDescriptor = jet_protocol::decode_control(&bytes)
			.map_err(std::io::Error::other)?;
		if descriptor.config.execution_id != id.0
			|| descriptor.role != "run"
			|| jet_runtime::execution_process_identity(descriptor.pid)?.as_ref()
				!= Some(&descriptor.process_start)
		{
			return Err(std::io::Error::other(
				"execution metadata no longer names a live helper",
			));
		}
		Ok(jet_core::ExecutionMetadata {
			instance: descriptor.instance,
			helper_pid: descriptor.pid,
			root: descriptor.config.working_directory,
			project_root: descriptor.config.project_directory,
			version: descriptor.version,
		})
	})
	.await?
	.map_err(run_host::failed)
}

pub(crate) struct Validated {
	runtime: PathBuf,
	socket: PathBuf,
	descriptor: HelperDescriptor,
}
/// Reads process evidence without requiring authoritative roots or an adoptable artifact.
/// Unsafe identity never becomes evidence of death.
pub(crate) async fn identity(
	home: PathBuf,
	id: RunId,
	expected_pid: Option<u32>,
) -> Result<HelperDescriptor, RunRecoveryError> {
	filesystem::blocking(move || {
		let path = home
			.join("runtime")
			.join(id.0.simple().to_string())
			.join("descriptor.json");
		let bytes = match jet_runtime::read_execution_file(&path) {
			Ok(bytes) => bytes,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				if let Some(pid) = expected_pid
					&& jet_runtime::execution_process_identity(pid).ok()
						== Some(None)
				{
					return Err(RunRecoveryError::Gone);
				}
				return Err(RunRecoveryError::Unsafe);
			}
			Err(_) => return Err(RunRecoveryError::Unsafe),
		};
		let descriptor: HelperDescriptor = jet_protocol::decode_control(&bytes)
			.map_err(|_| RunRecoveryError::Unsafe)?;
		if descriptor.role != "run"
			|| descriptor.config.execution_id != id.0
			|| expected_pid.is_some_and(|pid| pid != descriptor.pid)
		{
			return Err(RunRecoveryError::Unsafe);
		}
		let identity = jet_runtime::execution_process_identity(descriptor.pid)
			.map_err(|_| RunRecoveryError::Unsafe)?;
		if identity.is_none() {
			std::fs::remove_file(path).map_err(|_| RunRecoveryError::Unsafe)?;
			return Err(RunRecoveryError::Gone);
		}
		if identity.as_ref() != Some(&descriptor.process_start) {
			return Err(RunRecoveryError::Unsafe);
		}
		Ok(descriptor)
	})
	.await
	.map_err(|_| RunRecoveryError::Unsafe)?
}

pub(crate) async fn authenticated(
	home: PathBuf,
	id: RunId,
	expected_pid: Option<u32>,
) -> Result<
	(
		jet_protocol::FrameReader<tokio::net::unix::OwnedReadHalf>,
		jet_protocol::FrameWriter<tokio::net::unix::OwnedWriteHalf>,
		HelperDescriptor,
	),
	RunRecoveryError,
> {
	let mut descriptor = identity(home.clone(), id, expected_pid).await?;
	let directory = home.join("runtime").join(id.0.simple().to_string());
	let artifact = directory.join("jetfueld");
	let digest =
		filesystem::blocking(move || jet_runtime::execution_digest(&artifact))
			.await
			.map_err(|_| RunRecoveryError::Unsafe)?
			.map_err(|_| RunRecoveryError::Unsafe)?;
	if digest != descriptor.sha256 {
		return Err(RunRecoveryError::Unsafe);
	}
	let stream = UnixStream::connect(directory.join("h.sock"))
		.await
		.map_err(|_| RunRecoveryError::Unsafe)?;
	if stream
		.peer_cred()
		.map_err(|_| RunRecoveryError::Unsafe)?
		.uid() != rustix::process::geteuid().as_raw()
	{
		return Err(RunRecoveryError::Unsafe);
	}
	let (read, write) = stream.into_split();
	let mut reader = jet_protocol::FrameReader::new(read);
	let mut writer = jet_protocol::FrameWriter::new(write);
	run_host::send(
		&mut writer,
		&HelperHello {
			execution_id: id.0,
			protocol: ProtocolOffer {
				family: ProtocolFamily::Helper,
				versions: vec![ProtocolVersion { major: 1, minor: 1 }],
				capabilities: vec![],
			},
		},
	)
	.await
	.map_err(|_| RunRecoveryError::Unsafe)?;
	let ready: HelperReady =
		timeout(Duration::from_secs(10), run_host::receive(&mut reader))
			.await
			.map_err(|_| RunRecoveryError::Unsafe)?
			.map_err(|_| RunRecoveryError::Unsafe)?;
	descriptor.replay = ready.descriptor.replay.clone();
	// ASVS 8.3.1: ownership, retained artifact, OS start and the live peer must agree.
	if ready.descriptor != descriptor
		|| ready.helper_pid != descriptor.pid
		|| ready.version != (ProtocolVersion { major: 1, minor: 1 })
	{
		return Err(RunRecoveryError::Unsafe);
	}
	Ok((reader, writer, descriptor))
}

pub(crate) async fn validate_boot(
	plan: &LaunchPlan,
) -> Result<(), RunRecoveryError> {
	let accepted = crate::run_craft::Contract::of(&plan.craft)
		.map_err(|_| RunRecoveryError::Unsafe)?
		.boot_identity;
	if !accepted.is_empty() {
		let current =
			filesystem::blocking(jet_runtime::execution_boot_identity)
				.await
				.map_err(|_| RunRecoveryError::Unsafe)?
				.map_err(|_| RunRecoveryError::Unsafe)?;
		if accepted != current {
			return Err(RunRecoveryError::Gone);
		}
	}
	Ok(())
}

pub(crate) async fn validate(
	home: PathBuf,
	id: RunId,
	plan: &LaunchPlan,
	cursor: &RunRecoveryCursor,
) -> Result<Validated, RunRecoveryError> {
	validate_boot(plan).await?;
	let runtime = home.join("runtime");
	let socket = runtime.join(id.0.simple().to_string()).join("h.sock");
	let (_, mut writer, descriptor) =
		authenticated(home, id, cursor.helper_pid).await?;
	if descriptor.config
		!= run_host::helper_config(id, plan)
			.map_err(|_| RunRecoveryError::Unsafe)?
	{
		return Err(RunRecoveryError::Unsafe);
	}
	if cursor.offset != descriptor.replay.acknowledged
		&& Some(cursor.offset) != descriptor.replay.next_offset
	{
		return Err(RunRecoveryError::Unsafe);
	}
	run_host::send(&mut writer, &HelperCommand::Inspect)
		.await
		.map_err(|_| RunRecoveryError::Unsafe)?;
	plan.revalidate()
		.await
		.map_err(|_| RunRecoveryError::Unsafe)?;
	Ok(Validated {
		runtime,
		socket,
		descriptor,
	})
}

pub(crate) async fn connect(
	processes: &CraftProcesses,
	home: PathBuf,
	id: RunId,
	plan: LaunchPlan,
	cursor: RunRecoveryCursor,
) -> Result<Box<dyn jet_core::RunConnection>, RunRecoveryError> {
	let Validated {
		runtime,
		socket,
		descriptor,
	} = validate(home, id, &plan, &cursor).await?;
	let contract = crate::run_craft::Contract::of(&plan.craft)
		.map_err(|_| RunRecoveryError::Unsafe)?;
	if contract.craft_protocol.minor < 2 {
		return Err(RunRecoveryError::Unsafe);
	}
	let (mut reader, mut writer) =
		run_host::craft_connection(processes, &runtime, id, &plan)
			.await
			.map_err(|_| RunRecoveryError::Unavailable)?;
	run_host::send(
		&mut writer,
		&jet_protocol::CraftCommand::Recover {
			id: id.0.to_string(),
			helper_socket: socket.to_string_lossy().into_owned(),
			source_offset: cursor.offset,
			checkpoint: cursor.checkpoint,
		},
	)
	.await
	.map_err(|_| RunRecoveryError::Unavailable)?;
	let recovered: jet_protocol::CraftEvent =
		timeout(Duration::from_secs(10), run_host::receive(&mut reader))
			.await
			.map_err(|_| RunRecoveryError::Unavailable)?
			.map_err(|_| RunRecoveryError::Unavailable)?;
	if !matches!(recovered, jet_protocol::CraftEvent::RunRecovered { helper_pid, source_offset } if helper_pid == descriptor.pid && source_offset == cursor.offset)
	{
		return Err(RunRecoveryError::Unavailable);
	}
	Ok(Box::new(RunConnection {
		craft_minor: contract.craft_protocol.minor,
		reader: tokio::sync::Mutex::new(reader),
		writer: tokio::sync::Mutex::new(writer),
		helper_pid: descriptor.pid,
		run_id: id,
	}))
}

#[cfg(test)]
#[path = "run_recovery_tests.rs"]
mod tests;
