//! Escalation reaches a native process only through its own authenticated
//! helper instance, never a raw PID (ADR-0083).
use crate::run::host as run_host;
use jet_core::{CoreError, ExecutionSignal, RunId};
use jet_protocol::{HelperCommand, HelperSignalled, NativeSignal};
use std::{path::PathBuf, time::Duration};
use tokio::time::timeout;

pub(crate) async fn deliver(
	home: PathBuf,
	run_id: RunId,
	step: ExecutionSignal,
) -> Result<(), CoreError> {
	let metadata = crate::run::recovery::describe(home.clone(), run_id).await?;
	let (mut reader, mut writer, descriptor) =
		crate::run::recovery::authenticated(
			home,
			run_id,
			Some(metadata.helper_pid),
			crate::run::recovery::CONTROL_HELPER,
		)
		.await
		.map_err(|_| run_host::failed("unverified helper identity"))?;
	if descriptor.instance != metadata.instance {
		return Err(run_host::failed("stale execution instance"));
	}
	let signal = match step {
		ExecutionSignal::Interrupt => NativeSignal::Interrupt,
		ExecutionSignal::Terminate => NativeSignal::Terminate,
		ExecutionSignal::Kill => NativeSignal::Kill,
	};
	run_host::send(
		&mut writer,
		&HelperCommand::Signal {
			instance: metadata.instance,
			signal,
		},
	)
	.await?;
	let delivered: HelperSignalled =
		timeout(Duration::from_secs(10), run_host::receive(&mut reader))
			.await
			.map_err(run_host::failed)??;
	if delivered.instance != metadata.instance || delivered.signal != signal {
		return Err(run_host::failed("wrong signal acknowledgement"));
	}
	Ok(())
}
