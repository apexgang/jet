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

pub(super) fn action(value: core::ExecutionAction) -> wire::ExecutionAction {
	match value {
		core::ExecutionAction::Adopt => wire::ExecutionAction::Adopt,
		core::ExecutionAction::Leave => wire::ExecutionAction::Leave,
		core::ExecutionAction::Terminate => wire::ExecutionAction::Terminate,
	}
}
pub(super) fn action_from_wire(
	value: wire::ExecutionAction,
) -> core::ExecutionAction {
	match value {
		wire::ExecutionAction::Adopt => core::ExecutionAction::Adopt,
		wire::ExecutionAction::Leave => core::ExecutionAction::Leave,
		wire::ExecutionAction::Terminate => core::ExecutionAction::Terminate,
	}
}
pub(super) fn orphans(
	page: core::OrphanedExecutions,
) -> wire::OrphanedExecutions {
	wire::OrphanedExecutions {
		next: page.next.map(|id| id.0),
		executions: page
			.executions
			.into_iter()
			.map(|execution| wire::OrphanedExecution {
				execution_id: execution.execution_id.0,
				metadata: execution.metadata.map(|m| wire::ExecutionMetadata {
					instance: m.instance,
					helper_pid: m.helper_pid,
					root: m.root,
					project_root: m.project_root,
					version: m.version,
				}),
			})
			.collect(),
	}
}
