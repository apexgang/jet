//! Managed execution projections at the client protocol boundary.
use jet_core as core;
use jet_protocol as wire;

pub(super) fn execution(value: core::RunExecution) -> wire::RunExecution {
	wire::RunExecution {
		cursor: value.cursor.0,
		run: super::run(&value.run),
		activity: value.activity.map(activity),
		processes: value
			.processes
			.into_iter()
			.map(|process| wire::ManagedProcess {
				pid: process.pid,
				running: process.running,
				role: match process.role {
					core::ManagedProcessRole::Helper => {
						wire::ManagedProcessRole::Helper
					}
					core::ManagedProcessRole::Harness => {
						wire::ManagedProcessRole::Harness
					}
				},
			})
			.collect(),
		native_conversation: value.native_conversation,
		exit_code: value.exit_code,
	}
}

fn activity(value: core::RunActivity) -> wire::RunActivity {
	match value {
		core::RunActivity::Working => wire::RunActivity::Working,
		core::RunActivity::WaitingForUser => wire::RunActivity::WaitingForUser,
		core::RunActivity::WaitingForApproval => {
			wire::RunActivity::WaitingForApproval
		}
		core::RunActivity::WaitingForAuth => wire::RunActivity::WaitingForAuth,
		core::RunActivity::WaitingForQuota => {
			wire::RunActivity::WaitingForQuota
		}
		core::RunActivity::Reconnecting => wire::RunActivity::Reconnecting,
	}
}
