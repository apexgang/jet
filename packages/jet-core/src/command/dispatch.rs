//! Dispatches one prepared Command inside its existing write transaction.

use super::{
	Command, CommandId, CommandOutcome, clear_setting, create_conversation,
	create_run, set_setting, transition_run,
};
use crate::{
	Actor, Core, account,
	command::preparation::Prepared,
	conversation::{ConversationOrigin, import},
	error::CoreError,
	pairing::{
		self, completion as pairing_completion, offer as pairing_offer,
		paired_client,
	},
	project,
	promotion::command as promotion_command,
	security::{self, SecurityState},
	workspace::{self, WorkingTreeRequest, WorkspaceHome},
};
use jet_store::WriteTransaction;

/// What the core brings to a Command's transaction that neither the
/// Command nor its preparation carries.
pub(super) struct TransactionContext<'a> {
	/// Core services needed by trusted in-process adapters.
	pub(super) core: &'a Core,
	/// Whether the Plane vouched for its Security audit when the Command
	/// was admitted.
	pub(super) security: SecurityState,
	/// Where the Plane creates Workspaces.
	pub(super) workspace_home: &'a WorkspaceHome,
}

pub(super) async fn execute_new(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	command: Command,
	prepared: Prepared,
	context: TransactionContext<'_>,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let TransactionContext {
		core,
		security,
		workspace_home,
	} = context;
	match command {
		Command::SetAutoContinue { target, policy } => {
			crate::auto_continue::configure(
				tx,
				actor,
				target,
				policy,
				now_unix_ms,
			)
			.await
		}
		Command::AuthorizeApprovalRetry { run_id, review_id } => {
			crate::review::retry::authorize(
				tx,
				actor,
				run_id,
				review_id,
				now_unix_ms,
			)
			.await
		}
		Command::ReviewRemoteTool {
			client_id,
			operation_id,
			decision,
		} => {
			crate::remote::review::review(
				tx,
				actor,
				client_id,
				operation_id,
				decision,
				now_unix_ms,
			)
			.await
		}
		Command::AcknowledgeGitDelivery { delivery_id } => {
			crate::git_delivery::state::acknowledge(tx, actor, delivery_id)
				.await
		}
		Command::DeliverGit {
			conversation_id,
			checkpoint,
			operation,
		} => {
			crate::git_delivery::state::admit(
				tx,
				actor,
				command_id,
				conversation_id,
				checkpoint,
				operation,
			)
			.await
		}
		Command::RequestUtility { request } => {
			crate::utility::work::admit(tx, actor, command_id, request).await
		}
		Command::ChangeExtension { confirmation } => {
			crate::extension::work::admit(
				tx,
				actor,
				command_id,
				confirmation,
				now_unix_ms,
			)
			.await
		}
		Command::DisableCraft { craft_id, mode } => {
			crate::craft::lifecycle::record(
				tx,
				actor,
				craft_id,
				mode,
				now_unix_ms,
			)
			.await
		}
		Command::InstallCraft { .. } => {
			let Prepared::CraftInstallation(prepared) = prepared else {
				return Err(CoreError::internal(
					"craft.installation_unprepared",
					"a Craft installation reached its transaction without a verified Artifact",
				));
			};
			crate::craft::installation::record(
				tx,
				actor,
				command_id,
				prepared,
				now_unix_ms,
			)
			.await
		}
		Command::ApplyUserEdit { .. } => {
			let Prepared::UserEdit(prepared) = prepared else {
				return Err(CoreError::internal(
					"user_edit.unprepared",
					"a direct edit reached its transaction without preparation",
				));
			};
			crate::user_input::apply(
				core,
				tx,
				actor,
				command_id,
				prepared,
				now_unix_ms,
			)
			.await
		}
		Command::SubmitReview {
			conversation_id,
			comments,
		} => {
			crate::turn::queue::admit_review(
				tx,
				actor,
				command_id,
				conversation_id,
				comments,
				now_unix_ms,
			)
			.await
		}
		Command::SetConversationName {
			conversation_id,
			expected_revision,
			name,
		} => {
			crate::conversation::name::set_conversation(
				tx,
				actor,
				conversation_id,
				expected_revision,
				name,
				now_unix_ms,
			)
			.await
		}
		Command::SetRunName {
			run_id,
			expected_revision,
			name,
		} => {
			crate::conversation::name::set_run(
				tx,
				actor,
				run_id,
				expected_revision,
				name,
				now_unix_ms,
			)
			.await
		}
		Command::OpenTerminal { .. } => {
			let Prepared::Terminal(plan) = prepared else {
				return Err(crate::terminal::unavailable());
			};
			crate::terminal::command::open(
				tx,
				actor,
				command_id,
				plan,
				now_unix_ms,
			)
			.await
		}
		Command::WithdrawTurn {
			conversation_id,
			turn_id,
		} => {
			crate::turn::queue::withdraw(
				tx,
				actor,
				conversation_id,
				turn_id,
				now_unix_ms,
			)
			.await
		}
		Command::CloseTerminal { terminal_id } => {
			crate::terminal::command::close(
				tx,
				actor,
				command_id,
				terminal_id,
				now_unix_ms,
			)
			.await
		}
		Command::CreateSchedule {
			conversation_id,
			time_zone,
			local_time,
			prompt,
		} => {
			crate::schedule::create(
				tx,
				actor,
				conversation_id,
				time_zone,
				local_time,
				prompt,
				now_unix_ms,
			)
			.await
		}
		Command::CancelSchedule { schedule_id } => {
			crate::schedule::cancel(tx, actor, schedule_id, now_unix_ms).await
		}
		Command::SubmitTurn {
			conversation_id,
			source,
			prompt,
		} => {
			crate::turn::queue::admit(
				tx,
				actor,
				command_id,
				conversation_id,
				source,
				prompt,
				now_unix_ms,
			)
			.await
		}
		Command::ResolveExecution(request) => {
			crate::run::orphan::record(
				tx,
				actor,
				command_id,
				request,
				now_unix_ms,
			)
			.await
		}
		Command::ControlRun { run_id, control } => {
			crate::run::execution_control::record(
				tx,
				actor,
				command_id,
				run_id,
				control,
				now_unix_ms,
			)
			.await
		}
		Command::StartRun {
			conversation_id, ..
		}
		| Command::StartNoVisaRun(crate::NoVisaRunRequest {
			conversation_id,
			..
		})
		| Command::StartVisaRun(crate::VisaRunRequest {
			conversation_id,
			..
		}) => {
			let Prepared::Run(plan) = prepared else {
				return Err(CoreError::internal(
					"run.unprepared",
					"missing launch plan",
				));
			};
			crate::run::command::record(
				core,
				tx,
				actor,
				command_id,
				conversation_id,
				plan,
				now_unix_ms,
			)
			.await
		}
		Command::PromoteWorkspace { .. } => {
			let Prepared::Promotion(prepared) = prepared else {
				return Err(CoreError::internal(
					"workspace.promotion_unprepared",
					"a Workspace promotion reached its transaction without \
					 its revalidated binding",
				));
			};
			promotion_command::record(
				tx,
				actor,
				command_id,
				prepared,
				now_unix_ms,
			)
			.await
		}
		Command::RegisterProject { .. } => {
			let Prepared::Registration(registrable) = prepared else {
				return Err(CoreError::internal(
					"project.unprepared",
					"a Project registration reached its transaction without \
					 its prepared root",
				));
			};
			project::register(tx, actor, registrable, now_unix_ms).await
		}
		Command::CreateConversation {
			retention,
			working_tree,
		} => match working_tree {
			WorkingTreeRequest::NoProject => {
				create_conversation(tx, actor, retention, now_unix_ms).await
			}
			WorkingTreeRequest::Workspace { .. } => {
				let Prepared::Workspace(prepared) = prepared else {
					return Err(CoreError::internal(
						"workspace.unprepared",
						"a Workspace creation reached its transaction without \
						 its resolved base",
					));
				};
				workspace::create(
					tx,
					actor,
					retention,
					ConversationOrigin::New,
					prepared,
					workspace_home,
					now_unix_ms,
				)
				.await
			}
			WorkingTreeRequest::LocalCheckout { project_id } => {
				workspace::create_in_local_checkout(
					tx,
					actor,
					retention,
					ConversationOrigin::New,
					project_id,
					now_unix_ms,
				)
				.await
			}
		},
		Command::HandoffConversation(_) => {
			let Prepared::Handoff(prepared) = prepared else {
				return Err(CoreError::internal(
					"handoff.unprepared",
					"Handoff was not prepared",
				));
			};
			crate::conversation::handoff::create(
				core,
				tx,
				actor,
				command_id,
				*prepared,
				workspace_home,
				now_unix_ms,
			)
			.await
		}
		Command::ForkConversation { .. } => {
			let Prepared::Fork(prepared) = prepared else {
				return Err(CoreError::internal(
					"fork.unprepared",
					"a Conversation fork reached its transaction without its selected checkpoint",
				));
			};
			crate::conversation::fork::create(
				tx,
				actor,
				*prepared,
				workspace_home,
				now_unix_ms,
			)
			.await
		}
		Command::ImportConversation { .. } => {
			import::import(tx, actor, prepared, now_unix_ms).await
		}
		Command::ResumeImportedConversation {
			import_id,
			retention,
			working_tree,
		} => {
			import::resume(
				tx,
				actor,
				import::Resume {
					import_id,
					retention,
					working_tree,
					prepared,
				},
				workspace_home,
				now_unix_ms,
			)
			.await
		}
		Command::CreateRun { conversation_id } => {
			create_run(tx, actor, conversation_id, now_unix_ms).await
		}
		Command::SetSetting { key, scope, value } => {
			set_setting(tx, actor, key, scope, value, now_unix_ms).await
		}
		Command::ClearSetting { key, scope } => {
			clear_setting(tx, actor, key, scope, now_unix_ms).await
		}
		Command::BindAccount {
			provider,
			label,
			provider_account,
			credential_source,
		} => {
			account::bind(
				tx,
				actor,
				account::Requested {
					provider,
					label,
					provider_account,
					credential_source,
				},
				now_unix_ms,
			)
			.await
		}
		Command::UnbindAccount { binding_id } => {
			account::unbind(tx, actor, binding_id, now_unix_ms).await
		}
		Command::BeginAuditEpoch => {
			security::begin_epoch(tx, actor, security, now_unix_ms).await
		}
		// Intercepted before the pipeline in `Core::execute`; a serving
		// Plane answers the same way it would there.
		Command::RestoreRecoverySnapshot { .. } => {
			Err(crate::store_recovery::not_read_only())
		}
		// Intercepted before the pipeline as well; it never gets here.
		Command::PurgeRecoverySnapshots => Err(CoreError::internal(
			"recovery.purge_misrouted",
			"a Recovery purge reached the receipt pipeline",
		)),
		Command::SetPairingGate { gate } => {
			pairing::set_gate(tx, actor, gate, now_unix_ms).await
		}
		Command::OpenPairing { method } => {
			pairing_offer::open(tx, actor, method, now_unix_ms).await
		}
		Command::ClaimPairing { secret, key } => {
			pairing_offer::claim(tx, actor, secret, key, now_unix_ms).await
		}
		Command::ConfirmPairing {
			offer_id,
			authentication_string,
		} => {
			pairing_completion::confirm(
				tx,
				actor,
				offer_id,
				authentication_string,
				now_unix_ms,
			)
			.await
		}
		Command::SetPairedClientAccess { client_id, access } => {
			paired_client::set_access(tx, actor, client_id, access, now_unix_ms)
				.await
		}
		Command::RevokePairedClient { client_id } => {
			paired_client::revoke(tx, actor, client_id, now_unix_ms).await
		}
		Command::CompletePairing {
			offer_id,
			signature,
		} => {
			pairing_completion::complete(
				tx,
				actor,
				offer_id,
				signature,
				now_unix_ms,
			)
			.await
		}
		Command::TransitionRun {
			run_id,
			expected_revision,
			lifecycle,
		} => {
			transition_run(
				tx,
				actor,
				command_id,
				run_id,
				expected_revision,
				lifecycle,
				now_unix_ms,
			)
			.await
		}
	}
}
