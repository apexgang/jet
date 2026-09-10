//! Explicit domain/wire translation for Git delivery.
use jet_core as core;
use jet_protocol as wire;

pub(super) fn operation(value: core::GitOperation) -> wire::GitOperation {
	match value {
		core::GitOperation::Branch { name } => {
			wire::GitOperation::Branch { name }
		}
		core::GitOperation::Commit => wire::GitOperation::Commit,
		core::GitOperation::Push { remote } => {
			wire::GitOperation::Push { remote }
		}
		core::GitOperation::DraftPullRequest { remote, base } => {
			wire::GitOperation::DraftPullRequest { remote, base }
		}
	}
}
pub(super) fn operation_from_wire(
	value: wire::GitOperation,
) -> core::GitOperation {
	match value {
		wire::GitOperation::Branch { name } => {
			core::GitOperation::Branch { name }
		}
		wire::GitOperation::Commit => core::GitOperation::Commit,
		wire::GitOperation::Push { remote } => {
			core::GitOperation::Push { remote }
		}
		wire::GitOperation::DraftPullRequest { remote, base } => {
			core::GitOperation::DraftPullRequest { remote, base }
		}
	}
}
pub(super) fn checkpoint(value: wire::GitCheckpoint) -> core::GitCheckpoint {
	core::GitCheckpoint {
		run_id: core::RunId(value.run_id),
		turn: value.turn,
	}
}
pub(super) fn delivery(value: core::GitDelivery) -> wire::GitDelivery {
	wire::GitDelivery {
		message: value.message.map(|message| wire::GitMessage {
			title: message.title,
			body: message.body,
			fallback_reason: message.fallback_reason,
		}),
		acknowledged_by: value.acknowledged_by.map(|id| id.0),
		delivery_id: value.delivery_id,
		conversation_id: value.conversation_id.0,
		checkpoint: value.checkpoint.map(|c| wire::GitCheckpoint {
			run_id: c.run_id.0,
			turn: c.turn,
		}),
		operation: operation(value.operation),
		policy: wire::GitDeliveryPolicy {
			automatic: value.policy.automatic,
			branch: value.policy.branch,
			commit: value.policy.commit,
			push: value.policy.push,
			draft_pull_request: value.policy.draft_pull_request,
			branch_prefix: value.policy.branch_prefix,
		},
		utility_job: value.utility_job,
		outcome: match value.outcome {
			core::GitDeliveryOutcome::Pending => {
				wire::GitDeliveryOutcome::Pending
			}
			core::GitDeliveryOutcome::OutcomeUnknown => {
				wire::GitDeliveryOutcome::OutcomeUnknown
			}
			core::GitDeliveryOutcome::Failed { code } => {
				wire::GitDeliveryOutcome::Failed { code }
			}
			core::GitDeliveryOutcome::Completed {
				head,
				branch,
				pull_request,
			} => wire::GitDeliveryOutcome::Completed {
				head,
				branch,
				pull_request,
			},
		},
	}
}
