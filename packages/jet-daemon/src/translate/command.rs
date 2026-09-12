//! Validated wire Commands converted to core Commands.

use super::{
	account, auto_continue, craft_installation, extension,
	file_revision_from_wire, file_target_from_wire, git_delivery,
	lifecycle_from_wire, pairing, promotion, retention_from_wire, run, setting,
	turn, utility, working_tree_request,
};
use jet_core::{
	AccountBindingId, AuthenticationString, ClientId, Command, ConversationId,
	CoreError, HarnessId, ImportId, NativeConversationId, PairingOfferId,
	PairingSecret, PairingSignature, PathGrant, ProviderId, RelativePath,
	Revision, RunId,
};
use jet_protocol as wire;
use std::path::PathBuf;

/// The core form of a Command.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when a relative path is not
/// one the core accepts, so the core never receives an unvalidated path
/// (ADR-0101).
pub(crate) fn command(
	request: &wire::CommandRequest,
) -> Result<Command, CoreError> {
	Ok(match request {
		wire::CommandRequest::DisableCraft { craft_id, mode } => {
			Command::DisableCraft {
				craft_id: craft_id.clone(),
				mode: match mode {
					wire::CraftDisableMode::Wait => {
						jet_core::CraftDisableMode::Wait
					}
					wire::CraftDisableMode::Force => {
						jet_core::CraftDisableMode::Force
					}
				},
			}
		}
		wire::CommandRequest::ChangeExtension { confirmation } => {
			Command::ChangeExtension {
				confirmation: extension::confirmation_from_wire(
					confirmation.clone(),
				),
			}
		}
		wire::CommandRequest::InstallCraft { confirmation } => {
			Command::InstallCraft {
				confirmation: craft_installation::confirmation_from_wire(
					confirmation,
				),
			}
		}
		wire::CommandRequest::ApplyUserEdit {
			target,
			path,
			expected_revision,
			content,
		} => Command::ApplyUserEdit {
			target: file_target_from_wire(*target),
			path: RelativePath::parse(path)?,
			expected_revision: file_revision_from_wire(expected_revision),
			content: content.clone(),
		},
		wire::CommandRequest::SubmitReview {
			conversation_id,
			comments,
		} => Command::SubmitReview {
			conversation_id: ConversationId(*conversation_id),
			comments: comments
				.iter()
				.map(|comment| {
					Ok(jet_core::ReviewComment {
						path: RelativePath::parse(&comment.path)?,
						line: comment.line,
						comment: comment.comment.clone(),
					})
				})
				.collect::<Result<_, CoreError>>()?,
		},
		wire::CommandRequest::SetConversationName {
			conversation_id,
			expected_revision,
			name,
		} => Command::SetConversationName {
			conversation_id: ConversationId(*conversation_id),
			expected_revision: Revision(*expected_revision),
			name: jet_core::Name::manual(name.clone())?,
		},
		wire::CommandRequest::SetRunName {
			run_id,
			expected_revision,
			name,
		} => Command::SetRunName {
			run_id: RunId(*run_id),
			expected_revision: Revision(*expected_revision),
			name: jet_core::Name::manual(name.clone())?,
		},
		wire::CommandRequest::OpenTerminal {
			workspace_id,
			rows,
			columns,
		} => Command::OpenTerminal {
			workspace_id: jet_core::WorkspaceId(*workspace_id),
			rows: *rows,
			columns: *columns,
		},
		wire::CommandRequest::CloseTerminal { terminal_id } => {
			Command::CloseTerminal {
				terminal_id: jet_core::TerminalId(*terminal_id),
			}
		}
		wire::CommandRequest::ResolveExecution {
			execution_id,
			instance,
			action,
		} => Command::ResolveExecution(jet_core::ExecutionResolution {
			execution_id: RunId(*execution_id),
			instance: *instance,
			action: run::action_from_wire(*action),
		}),
		wire::CommandRequest::WithdrawTurn {
			conversation_id,
			turn_id,
		} => Command::WithdrawTurn {
			conversation_id: ConversationId(*conversation_id),
			turn_id: *turn_id,
		},
		wire::CommandRequest::SetAutoContinue { target, policy } => {
			Command::SetAutoContinue {
				target: auto_continue::target(*target),
				policy: auto_continue::policy(policy.clone()),
			}
		}
		wire::CommandRequest::CreateSchedule {
			conversation_id,
			time_zone,
			local_time,
			prompt,
		} => Command::CreateSchedule {
			conversation_id: ConversationId(*conversation_id),
			time_zone: time_zone.clone(),
			local_time: local_time.clone(),
			prompt: prompt.clone(),
		},
		wire::CommandRequest::CancelSchedule { schedule_id } => {
			Command::CancelSchedule {
				schedule_id: *schedule_id,
			}
		}
		wire::CommandRequest::SubmitTurn {
			conversation_id,
			source,
			prompt,
		} => Command::SubmitTurn {
			conversation_id: ConversationId(*conversation_id),
			source: turn::source_from_wire(*source),
			prompt: prompt.clone(),
		},
		wire::CommandRequest::InterruptTurn { run_id } => Command::ControlRun {
			run_id: RunId(*run_id),
			control: jet_core::RunControl::InterruptTurn,
		},
		wire::CommandRequest::StopRun { run_id } => Command::ControlRun {
			run_id: RunId(*run_id),
			control: jet_core::RunControl::StopRun,
		},
		wire::CommandRequest::AuthorizeApprovalRetry { run_id, review_id } => {
			Command::AuthorizeApprovalRetry {
				run_id: RunId(*run_id),
				review_id: *review_id,
			}
		}
		wire::CommandRequest::ReviewRemoteTool {
			client_id,
			operation_id,
			decision,
		} => Command::ReviewRemoteTool {
			client_id: ClientId(*client_id),
			operation_id: *operation_id,
			decision: match decision {
				wire::RemoteToolDecision::AllowOnce => {
					jet_core::RemoteToolDecision::AllowOnce
				}
				wire::RemoteToolDecision::Deny => {
					jet_core::RemoteToolDecision::Deny
				}
			},
		},
		wire::CommandRequest::StartNoVisaRun(request) => {
			Command::StartNoVisaRun(jet_core::NoVisaRunRequest {
				conversation_id: jet_core::ConversationId(
					request.conversation_id,
				),
				origin_plane_id: jet_core::PlaneId(request.origin_plane_id),
				account_binding_id: jet_core::AccountBindingId(
					request.account_binding_id,
				),
				craft: request.craft.clone(),
				prompt: request.prompt.clone(),
				destinations: request
					.destinations
					.iter()
					.map(|d| jet_core::NoVisaDestination {
						plane_id: jet_core::PlaneId(d.plane_id),
						workspace_id: jet_core::WorkspaceId(d.workspace_id),
						ssh_endpoint: d.ssh_endpoint.clone(),
					})
					.collect(),
			})
		}
		wire::CommandRequest::StartVisaRun(request) => {
			Command::StartVisaRun(jet_core::VisaRunRequest {
				conversation_id: ConversationId(request.conversation_id),
				destination_plane_id: jet_core::PlaneId(
					request.destination_plane_id,
				),
				account_binding_id: jet_core::AccountBindingId(
					request.account_binding_id,
				),
				craft: request.craft.clone(),
				prompt: request.prompt.clone(),
			})
		}
		wire::CommandRequest::StartRun {
			conversation_id,
			craft,
			prompt,
		} => Command::StartRun {
			conversation_id: ConversationId(*conversation_id),
			craft: craft.clone(),
			prompt: prompt.clone(),
		},
		wire::CommandRequest::CreateConversation {
			retention,
			working_tree,
		} => Command::CreateConversation {
			retention: retention_from_wire(*retention),
			working_tree: working_tree_request(working_tree)?,
		},
		wire::CommandRequest::HandoffConversation(request) => {
			Command::HandoffConversation(jet_core::HandoffRequest {
				source_run_id: RunId(request.source_run_id),
				craft: request.craft.clone(),
				summary: request.summary.clone(),
				plan: request.plan.clone(),
				files: request
					.files
					.iter()
					.map(|path| jet_core::RelativePath::parse(path))
					.collect::<Result<_, _>>()?,
			})
		}
		wire::CommandRequest::ForkConversation {
			source_run_id,
			checkpoint_turn,
		} => Command::ForkConversation {
			source_run_id: RunId(*source_run_id),
			checkpoint_turn: *checkpoint_turn,
		},
		wire::CommandRequest::CreateRun { conversation_id } => {
			Command::CreateRun {
				conversation_id: ConversationId(*conversation_id),
			}
		}
		wire::CommandRequest::AcknowledgeGitDelivery { delivery_id } => {
			Command::AcknowledgeGitDelivery {
				delivery_id: *delivery_id,
			}
		}
		wire::CommandRequest::DeliverGit {
			conversation_id,
			checkpoint,
			operation,
		} => Command::DeliverGit {
			conversation_id: ConversationId(*conversation_id),
			checkpoint: checkpoint.map(git_delivery::checkpoint),
			operation: git_delivery::operation_from_wire(operation.clone()),
		},
		wire::CommandRequest::RequestUtility { request } => {
			Command::RequestUtility {
				request: utility::request(request.clone()),
			}
		}
		wire::CommandRequest::SetSetting { key, scope, value } => {
			Command::SetSetting {
				key: setting::key_from_wire(*key),
				scope: setting::scope_from_wire(*scope),
				value: setting::value_from_wire(value.clone()),
			}
		}
		wire::CommandRequest::ClearSetting { key, scope } => {
			Command::ClearSetting {
				key: setting::key_from_wire(*key),
				scope: setting::scope_from_wire(*scope),
			}
		}
		wire::CommandRequest::BindAccount {
			provider,
			label,
			provider_account,
			credential_source,
		} => Command::BindAccount {
			provider: ProviderId(provider.clone()),
			label: label.clone(),
			provider_account: account::provider_account(
				provider_account.as_ref(),
			),
			credential_source: account::source_from_wire(credential_source),
		},
		wire::CommandRequest::UnbindAccount { binding_id } => {
			Command::UnbindAccount {
				binding_id: AccountBindingId(*binding_id),
			}
		}
		wire::CommandRequest::BeginAuditEpoch => Command::BeginAuditEpoch,
		wire::CommandRequest::RestoreRecoverySnapshot { snapshot } => {
			Command::RestoreRecoverySnapshot {
				snapshot: snapshot.clone(),
			}
		}
		wire::CommandRequest::PurgeRecoverySnapshots => {
			Command::PurgeRecoverySnapshots
		}
		wire::CommandRequest::SetPairingGate { gate } => {
			Command::SetPairingGate {
				gate: pairing::gate_from_wire(*gate),
			}
		}
		wire::CommandRequest::OpenPairing { method } => Command::OpenPairing {
			method: pairing::method_from_wire(method),
		},
		wire::CommandRequest::ClaimPairing { secret, key } => {
			Command::ClaimPairing {
				secret: PairingSecret(secret.clone()),
				key: pairing::key_from_wire(key),
			}
		}
		wire::CommandRequest::ConfirmPairing {
			offer_id,
			authentication_string,
		} => Command::ConfirmPairing {
			offer_id: PairingOfferId(*offer_id),
			authentication_string: AuthenticationString(
				authentication_string.clone(),
			),
		},
		wire::CommandRequest::CompletePairing {
			offer_id,
			signature,
		} => Command::CompletePairing {
			offer_id: PairingOfferId(*offer_id),
			signature: PairingSignature(*signature),
		},
		wire::CommandRequest::SetPairedClientAccess { client_id, access } => {
			Command::SetPairedClientAccess {
				client_id: ClientId(*client_id),
				access: pairing::access_from_wire(*access),
			}
		}
		wire::CommandRequest::RevokePairedClient { client_id } => {
			Command::RevokePairedClient {
				client_id: ClientId(*client_id),
			}
		}
		wire::CommandRequest::TransitionRun {
			run_id,
			expected_revision,
			lifecycle,
		} => Command::TransitionRun {
			run_id: RunId(*run_id),
			expected_revision: Revision(*expected_revision),
			lifecycle: lifecycle_from_wire(*lifecycle),
		},
		wire::CommandRequest::RegisterProject { path } => {
			Command::RegisterProject {
				grant: PathGrant(PathBuf::from(path)),
			}
		}
		wire::CommandRequest::PromoteWorkspace { binding } => {
			Command::PromoteWorkspace {
				binding: promotion::binding_from_wire(binding),
			}
		}
		wire::CommandRequest::ImportConversation {
			harness,
			native_conversation,
		} => Command::ImportConversation {
			harness: HarnessId(harness.clone()),
			native_conversation: NativeConversationId(
				native_conversation.clone(),
			),
		},
		wire::CommandRequest::ResumeImportedConversation {
			import_id,
			retention,
			working_tree,
		} => Command::ResumeImportedConversation {
			import_id: ImportId(*import_id),
			retention: retention_from_wire(*retention),
			working_tree: working_tree_request(working_tree)?,
		},
	})
}
