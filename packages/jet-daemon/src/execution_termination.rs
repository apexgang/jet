//! Interactive termination uses an authenticated helper instance, never a raw PID.
use crate::run_host::{self, filesystem};
use jet_core::{CoreError, ExecutionResolution};
use jet_protocol::{HelperCommand, HelperTerminated};
use std::{path::PathBuf, time::Duration};
use tokio::time::timeout;

pub(crate) async fn terminate(
	home: PathBuf,
	request: ExecutionResolution,
) -> Result<(), CoreError> {
	let metadata =
		crate::run_recovery::describe(home.clone(), request.execution_id)
			.await?;
	if Some(metadata.instance) != request.instance {
		return Err(run_host::failed("stale execution instance"));
	}
	let (mut reader, mut writer, descriptor) =
		crate::run_recovery::authenticated(
			home,
			request.execution_id,
			Some(metadata.helper_pid),
		)
		.await
		.map_err(|_| run_host::failed("unverified helper identity"))?;
	if descriptor.instance != metadata.instance {
		return Err(run_host::failed("stale execution instance"));
	}
	run_host::send(
		&mut writer,
		&HelperCommand::Terminate {
			instance: metadata.instance,
		},
	)
	.await?;
	let stopped: HelperTerminated =
		timeout(Duration::from_secs(10), run_host::receive(&mut reader))
			.await
			.map_err(run_host::failed)??;
	if stopped.instance != metadata.instance {
		return Err(run_host::failed("wrong termination acknowledgement"));
	}
	// Confirm the helper itself exited before reporting the resolution complete.
	timeout(Duration::from_secs(10), async {
		loop {
			if filesystem::blocking(move || {
				jet_runtime::execution_process_identity(metadata.helper_pid)
			})
			.await?
			.map_err(run_host::failed)?
			.is_none()
			{
				return Ok(());
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.map_err(run_host::failed)?
}
