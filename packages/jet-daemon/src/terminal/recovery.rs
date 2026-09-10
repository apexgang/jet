//! Discover terminal helpers independently of authoritative database records.
use crate::terminal::host::{exchange, failed};
use jet_core::{
	CoreError, ExecutionMetadata, ExecutionResolution, TerminalId,
	TerminalOperation,
};
use jet_protocol::TerminalDescriptor;
use std::{io, path::PathBuf, time::Duration};

fn directory(home: &std::path::Path, id: TerminalId) -> PathBuf {
	home.join("runtime").join(format!("t-{}", id.0.simple()))
}
pub(crate) async fn discover(
	home: PathBuf,
) -> Result<Vec<TerminalId>, CoreError> {
	tokio::task::spawn_blocking(move || {
		let runtime = home.join("runtime");
		if !runtime.exists() {
			return Ok(vec![]);
		}
		jet_runtime::validate_execution_directory(&runtime)?;
		let mut ids = Vec::new();
		let boot = boot()?;
		for entry in std::fs::read_dir(runtime)? {
			let entry = entry?;
			if let Some(name) = entry
				.file_name()
				.to_str()
				.and_then(|s| s.strip_prefix("t-"))
				&& let Ok(id) = uuid::Uuid::parse_str(name)
				&& entry
					.path()
					.join("descriptor.json")
					.symlink_metadata()
					.is_ok()
			{
				let descriptor = jet_runtime::read_execution_file(
					&entry.path().join("descriptor.json"),
				)
				.ok()
				.and_then(|bytes| {
					jet_protocol::decode_control::<TerminalDescriptor>(&bytes)
						.ok()
				});
				if let Some(descriptor) = descriptor
					&& descriptor.role == "terminal"
					&& descriptor.config.terminal_id == id
					&& (descriptor.config.boot != boot
						|| jet_runtime::execution_process_identity(
							descriptor.pid,
						)
						.ok() == Some(None))
				{
					continue;
				}
				ids.push(TerminalId(id));
				if ids.len() > 4096 {
					return Err(io::Error::other(
						"too many terminal executions",
					));
				}
			}
		}
		Ok(ids)
	})
	.await
	.map_err(failed)?
	.map_err(failed)
}
async fn identity(
	home: PathBuf,
	id: TerminalId,
) -> Result<Option<TerminalDescriptor>, CoreError> {
	tokio::task::spawn_blocking(move || {
		let directory = directory(&home, id);
		let descriptor: TerminalDescriptor =
			jet_protocol::decode_control(&jet_runtime::read_execution_file(
				&directory.join("descriptor.json"),
			)?)
			.map_err(io::Error::other)?;
		if descriptor.protocol
			!= (jet_protocol::ProtocolVersion { major: 1, minor: 0 })
			|| descriptor.role != "terminal"
			|| descriptor.config.terminal_id != id.0
		{
			return Err(io::Error::other("terminal identity mismatch"));
		}
		if descriptor.config.boot != boot()? {
			return Ok(None);
		}
		match jet_runtime::execution_process_identity(descriptor.pid)? {
			None => return Ok(None),
			Some(identity) if identity == descriptor.process_start => {}
			Some(_) => {
				return Err(io::Error::other("terminal process mismatch"));
			}
		}
		if jet_runtime::execution_digest(&directory.join("jetfueld"))?
			!= descriptor.sha256
		{
			return Err(io::Error::other("terminal artifact mismatch"));
		}
		Ok(Some(descriptor))
	})
	.await
	.map_err(failed)?
	.map_err(failed)
}
pub(crate) async fn inspect(
	home: PathBuf,
	id: TerminalId,
) -> Result<Option<ExecutionMetadata>, CoreError> {
	let Some(descriptor) = identity(home.clone(), id).await? else {
		return Ok(None);
	};
	let output = tokio::time::timeout(
		Duration::from_secs(10),
		exchange(
			directory(&home, id),
			descriptor.clone(),
			TerminalOperation::Read { after: 0, limit: 0 },
		),
	)
	.await
	.map_err(failed)??;
	if output.closed {
		return Ok(None);
	}
	Ok(Some(ExecutionMetadata {
		instance: descriptor.instance,
		helper_pid: descriptor.pid,
		root: descriptor.config.root,
		project_root: descriptor.config.project_root,
		version: descriptor.version,
	}))
}
pub(crate) async fn terminate(
	home: PathBuf,
	request: ExecutionResolution,
) -> Result<(), CoreError> {
	let id = TerminalId(request.execution_id.0);
	let descriptor = identity(home.clone(), id)
		.await?
		.ok_or_else(|| failed("terminal gone"))?;
	if request.instance != Some(descriptor.instance) {
		return Err(failed("terminal instance changed"));
	}
	let output = tokio::time::timeout(
		Duration::from_secs(10),
		exchange(directory(&home, id), descriptor, TerminalOperation::Close),
	)
	.await
	.map_err(failed)??;
	if !output.closed {
		return Err(failed("terminal not closed"));
	}
	Ok(())
}

// A process cannot cross an OS reboot. Avoid spawning sysctl for each PTY read.
pub(crate) fn boot() -> io::Result<String> {
	static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
	if let Some(boot) = BOOT.get() {
		return Ok(boot.clone());
	}
	let boot = jet_runtime::execution_boot_identity()?;
	let _ = BOOT.set(boot.clone());
	Ok(boot)
}
