//! Committed Command outcomes converted to wire replies.

use super::{
	account, conversation, execution_control, file_revision, file_target,
	import, pairing, project, promotion, run, schedule, setting, terminal,
	turn,
};
use jet_core::CommandOutcome;
use jet_protocol as wire;

pub(crate) fn command_outcome(
	outcome: CommandOutcome,
	minor: u32,
) -> wire::CommandResponse {
	match outcome {
		CommandOutcome::GitDeliveryAcknowledged { delivery_id } => {
			wire::CommandResponse::GitDeliveryAcknowledged { delivery_id }
		}
		CommandOutcome::GitDeliveryQueued { delivery_id } => {
			wire::CommandResponse::GitDeliveryQueued { delivery_id }
		}
		CommandOutcome::UtilityQueued { job_id } => {
			wire::CommandResponse::UtilityQueued { job_id }
		}
		CommandOutcome::CraftDisabled { craft_id, mode } => {
			wire::CommandResponse::CraftDisabled {
				craft_id,
				mode: match mode {
					jet_core::CraftDisableMode::Wait => {
						wire::CraftDisableMode::Wait
					}
					jet_core::CraftDisableMode::Force => {
						wire::CraftDisableMode::Force
					}
				},
			}
		}
		CommandOutcome::ExtensionChangeQueued { change_id } => {
			wire::CommandResponse::ExtensionChangeQueued { change_id }
		}
		CommandOutcome::CraftInstallationQueued {
			craft_id,
			version,
			artifact_sha256,
		} => wire::CommandResponse::CraftInstallationQueued(
			wire::CraftInstallationQueued {
				craft_id,
				version,
				artifact_sha256,
			},
		),
		CommandOutcome::UserEditApplied(edit) => {
			wire::CommandResponse::UserEditApplied {
				target: file_target(edit.target),
				path: edit.path.as_str().into(),
				revision: file_revision(edit.revision),
			}
		}
		CommandOutcome::ConversationNamed(named) => {
			wire::CommandResponse::ConversationNamed(conversation(
				&named, minor,
			))
		}
		CommandOutcome::RunNamed(named) => {
			wire::CommandResponse::RunNamed(run(&named, minor))
		}
		CommandOutcome::Terminal(value) => wire::CommandResponse::Terminal {
			terminal: terminal::snapshot(value),
		},
		CommandOutcome::ExecutionResolutionRecorded(request) => {
			wire::CommandResponse::ExecutionResolutionRecorded {
				execution_id: request.execution_id.0,
				action: run::action(request.action),
			}
		}
		CommandOutcome::RunControlAccepted {
			run: value,
			control,
		} => wire::CommandResponse::RunControlAccepted {
			run: run(&value, minor),
			control: execution_control::control(control),
		},
		CommandOutcome::TurnWithdrawn(value) => {
			wire::CommandResponse::TurnWithdrawn {
				turn: turn::turn(value),
			}
		}
		CommandOutcome::AutoContinueConfigured => {
			wire::CommandResponse::AutoContinueConfigured
		}
		CommandOutcome::ScheduleCreated(value) => {
			wire::CommandResponse::ScheduleCreated {
				task: schedule::task(value),
			}
		}
		CommandOutcome::ScheduleCanceled { schedule_id } => {
			wire::CommandResponse::ScheduleCanceled { schedule_id }
		}
		CommandOutcome::TurnAdmitted(value) => {
			wire::CommandResponse::TurnAdmitted {
				turn: turn::turn(value),
			}
		}
		CommandOutcome::ConversationCreated(created) => {
			wire::CommandResponse::ConversationCreated(conversation(
				&created, minor,
			))
		}
		CommandOutcome::RunCreated(created) => {
			wire::CommandResponse::RunCreated(run(&created, minor))
		}
		CommandOutcome::RunTransitioned(transitioned) => {
			wire::CommandResponse::RunTransitioned(run(&transitioned, minor))
		}
		CommandOutcome::SettingSet { key, scope, value } => {
			wire::CommandResponse::SettingSet {
				key: setting::key(key),
				scope: setting::scope(scope),
				value: setting::value(value),
			}
		}
		CommandOutcome::SettingCleared { key, scope } => {
			wire::CommandResponse::SettingCleared {
				key: setting::key(key),
				scope: setting::scope(scope),
			}
		}
		CommandOutcome::ApprovalRetryAuthorized { review_id } => {
			wire::CommandResponse::ApprovalRetryAuthorized { review_id }
		}
		CommandOutcome::RemoteToolReviewed { operation_id } => {
			wire::CommandResponse::RemoteToolReviewed { operation_id }
		}
		CommandOutcome::AccountBound(bound) => {
			wire::CommandResponse::AccountBound(account::binding(bound))
		}
		CommandOutcome::AccountUnbound {
			binding_id,
			credential_reference,
		} => wire::CommandResponse::AccountUnbound {
			binding_id: binding_id.0,
			credential_reference: account::reference(credential_reference),
		},
		CommandOutcome::AuditEpochBegun { epoch } => {
			wire::CommandResponse::AuditEpochBegun { epoch: epoch.0 }
		}
		CommandOutcome::PairingGateSet { gate } => {
			wire::CommandResponse::PairingGateSet {
				gate: pairing::gate(gate),
			}
		}
		CommandOutcome::PairingOpened {
			pending,
			disclosure,
		} => wire::CommandResponse::PairingOpened {
			pending: pairing::pending(pending),
			disclosure: pairing::disclosure(disclosure),
		},
		CommandOutcome::PairingClaimed { pending, challenge } => {
			wire::CommandResponse::PairingClaimed {
				pending: pairing::pending(pending),
				challenge: challenge.0,
			}
		}
		CommandOutcome::PairingConfirmed { pending } => {
			wire::CommandResponse::PairingConfirmed {
				pending: pairing::pending(pending),
			}
		}
		CommandOutcome::PairingCompleted { client } => {
			wire::CommandResponse::PairingCompleted {
				client: pairing::client(client),
			}
		}
		CommandOutcome::PairedClientAccessSet { client } => {
			wire::CommandResponse::PairedClientAccessSet {
				client: pairing::client(client),
			}
		}
		CommandOutcome::PairedClientRevoked { client_id } => {
			wire::CommandResponse::PairedClientRevoked {
				client_id: client_id.0,
			}
		}
		CommandOutcome::ProjectRegistered(project) => {
			wire::CommandResponse::ProjectRegistered(project::project(project))
		}
		CommandOutcome::WorkspacePromotionRecorded(recorded) => {
			wire::CommandResponse::WorkspacePromotionRecorded(
				promotion::promotion(recorded),
			)
		}
		CommandOutcome::ConversationImported(imported) => {
			wire::CommandResponse::ConversationImported(import::imported(
				imported,
			))
		}
	}
}
