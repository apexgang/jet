//! Translation between core domain types and versioned wire types
//! (ADR-0049). This is the only place the two vocabularies meet; its
//! Account binding, Capability, Pairing, Project, and Setting parts sit in
//! the submodules beside it.

mod account;
mod audit;
mod capability;
mod pairing;
mod project;
mod setting;

pub(crate) use capability::snapshot as capabilities;
pub(crate) use pairing::{client as paired_client, pending as pairing_pending};

use std::time::{SystemTime, UNIX_EPOCH};

use std::path::PathBuf;

use jet_core::{
	AccountBindingId, Actor, AuditSequence, AuthenticationString,
	BaseSelection, ClientId, Command, CommandOutcome, ConflictState,
	Conversation, ConversationId, ConversationList, ConversationSnapshot,
	CoreError, ErrorCategory, Event, EventPage, EventPayload, EventSequence,
	PairingOfferId, PairingSecret, PairingSignature, PathGrant, PlaneStatus,
	ProjectId, ProviderId, Query, QueryResult, RecoveryAction, RelativePath,
	RetentionPolicy, Revision, RevisionConflict, Run, RunId, RunLifecycle,
	WorkingTree, WorkingTreeRequest, Workspace, WorkspaceBase,
};
use jet_protocol as wire;

/// The core form of a Query.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when a relative path is not
/// one the core accepts, so the core never receives an unvalidated path
/// (ADR-0101).
pub(crate) fn query(
	request: &wire::QueryRequest,
	minor: u32,
) -> Result<Query, CoreError> {
	Ok(match request {
		wire::QueryRequest::Status => Query::Status,
		wire::QueryRequest::Conversations
			if minor < wire::FENCED_READS_MINOR =>
		{
			Query::LegacyConversations
		}
		wire::QueryRequest::Conversations => Query::Conversations,
		wire::QueryRequest::NextConversations { cursor } => {
			Query::NextConversations {
				cursor: jet_core::PageCursor(cursor.0),
			}
		}
		wire::QueryRequest::Conversation { conversation_id } => {
			Query::Conversation {
				conversation_id: ConversationId(*conversation_id),
			}
		}
		wire::QueryRequest::Capabilities { observation } => {
			Query::Capabilities {
				observation: capability::observation(*observation),
			}
		}
		wire::QueryRequest::AccountBindings { observation } => {
			Query::AccountBindings {
				observation: capability::observation(*observation),
			}
		}
		wire::QueryRequest::Settings { scope, selection } => Query::Settings {
			scope: setting::scope_from_wire(*scope),
			selection: setting::selection_from_wire(*selection),
		},
		wire::QueryRequest::Events { after } => Query::Events {
			after: EventSequence(*after),
		},
		wire::QueryRequest::Pairing => Query::Pairing,
		wire::QueryRequest::SecurityAudit { after } => Query::SecurityAudit {
			after: AuditSequence(*after),
		},
		wire::QueryRequest::Projects => Query::Projects,
		wire::QueryRequest::PreviewProject { path, observation } => {
			Query::PreviewProject {
				grant: PathGrant(PathBuf::from(path)),
				observation: capability::observation(*observation),
			}
		}
		wire::QueryRequest::ProjectEntry { project_id, path } => {
			Query::ProjectEntry {
				project_id: ProjectId(*project_id),
				path: RelativePath::parse(path)?,
			}
		}
	})
}

pub(crate) fn query_result(
	result: QueryResult,
	minor: u32,
) -> Result<wire::QueryResponse, CoreError> {
	Ok(match result {
		QueryResult::Status(status) => {
			wire::QueryResponse::Status(plane_status(&status, minor))
		}
		QueryResult::Conversations(list) => {
			wire::QueryResponse::Conversations(conversation_list(&list, minor))
		}
		QueryResult::Conversation(snapshot) => {
			wire::QueryResponse::Conversation(conversation_snapshot(
				&snapshot, minor,
			))
		}
		QueryResult::Capabilities(snapshot) => {
			wire::QueryResponse::Capabilities(capability::snapshot(
				snapshot, minor,
			))
		}
		QueryResult::AccountBindings(bindings) => {
			wire::QueryResponse::AccountBindings(account::list(bindings))
		}
		QueryResult::Settings(snapshot) => {
			wire::QueryResponse::Settings(setting::snapshot(snapshot, minor))
		}
		QueryResult::Events(page) => {
			wire::QueryResponse::Events(event_page(&page)?)
		}
		QueryResult::Pairing(snapshot) => {
			wire::QueryResponse::Pairing(pairing::snapshot(snapshot))
		}
		QueryResult::SecurityAudit(page) => {
			wire::QueryResponse::SecurityAudit(audit::page(page))
		}
		QueryResult::Projects(list) => {
			wire::QueryResponse::Projects(project::list(list))
		}
		QueryResult::ProjectPreview(preview) => {
			wire::QueryResponse::ProjectPreview(project::preview(preview))
		}
		QueryResult::ProjectEntry(entry) => {
			wire::QueryResponse::ProjectEntry(project::entry(entry))
		}
	})
}

pub(crate) fn command(request: &wire::CommandRequest) -> Command {
	match request {
		wire::CommandRequest::CreateConversation {
			retention,
			working_tree,
		} => Command::CreateConversation {
			retention: retention_from_wire(*retention),
			working_tree: working_tree_request(working_tree),
		},
		wire::CommandRequest::CreateRun { conversation_id } => {
			Command::CreateRun {
				conversation_id: ConversationId(*conversation_id),
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
	}
}

pub(crate) fn command_outcome(
	outcome: CommandOutcome,
	minor: u32,
) -> wire::CommandResponse {
	match outcome {
		CommandOutcome::ConversationCreated(created) => {
			wire::CommandResponse::ConversationCreated(conversation(
				&created, minor,
			))
		}
		CommandOutcome::RunCreated(created) => {
			wire::CommandResponse::RunCreated(run(&created))
		}
		CommandOutcome::RunTransitioned(transitioned) => {
			wire::CommandResponse::RunTransitioned(run(&transitioned))
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
	}
}

fn plane_status(status: &PlaneStatus, minor: u32) -> wire::PlaneStatus {
	wire::PlaneStatus {
		cursor: (minor >= wire::FENCED_READS_MINOR).then_some(status.cursor.0),
		plane_id: status.plane_id.0,
		daemon_starts: status.daemon_starts,
		started_at_unix_ms: unix_ms(status.started_at),
		core_version: status.core_version.into(),
		// A client that negotiated an older minor does not name the
		// Security audit, so it is not told about its state either
		// (ADR-0019).
		security: (minor >= wire::SECURITY_AUDIT_MINOR)
			.then(|| audit::security(status.security)),
	}
}

fn conversation_list(
	list: &ConversationList,
	minor: u32,
) -> wire::ConversationList {
	wire::ConversationList {
		cursor: list.cursor.0,
		conversations: list
			.conversations
			.iter()
			.map(|conversation| self::conversation(conversation, minor))
			.collect(),
		next_page: list
			.next_page
			.as_ref()
			.map(|cursor| wire::PageCursor(cursor.0)),
	}
}

fn conversation_snapshot(
	snapshot: &ConversationSnapshot,
	minor: u32,
) -> wire::ConversationSnapshot {
	wire::ConversationSnapshot {
		cursor: snapshot.cursor.0,
		conversation: conversation(&snapshot.conversation, minor),
		// A client that negotiated an older minor is not told about a
		// Workspace it cannot decode (ADR-0019).
		workspace: (minor >= wire::WORKSPACES_MINOR)
			.then(|| snapshot.workspace.as_ref().map(workspace))
			.flatten(),
		runs: snapshot.runs.iter().map(run).collect(),
	}
}

fn conversation(conversation: &Conversation, minor: u32) -> wire::Conversation {
	wire::Conversation {
		conversation_id: conversation.conversation_id.0,
		retention: retention(conversation.retention),
		working_tree: (minor >= wire::WORKSPACES_MINOR)
			.then(|| working_tree(conversation.working_tree)),
		created_at_unix_ms: unix_ms(conversation.created_at),
	}
}

fn working_tree(working_tree: WorkingTree) -> wire::WorkingTree {
	match working_tree {
		WorkingTree::NoProject => wire::WorkingTree::NoProject,
		WorkingTree::Workspace { project_id } => wire::WorkingTree::Workspace {
			project_id: project_id.0,
		},
		WorkingTree::LocalCheckout { project_id } => {
			wire::WorkingTree::LocalCheckout {
				project_id: project_id.0,
			}
		}
	}
}

fn working_tree_request(
	request: &wire::WorkingTreeRequest,
) -> WorkingTreeRequest {
	match request {
		wire::WorkingTreeRequest::NoProject => WorkingTreeRequest::NoProject,
		wire::WorkingTreeRequest::Workspace { project_id, base } => {
			WorkingTreeRequest::Workspace {
				project_id: ProjectId(*project_id),
				base: match base {
					wire::BaseSelection::Head => BaseSelection::Head,
					wire::BaseSelection::Revision { revision } => {
						BaseSelection::Revision(revision.clone())
					}
				},
			}
		}
		wire::WorkingTreeRequest::LocalCheckout { project_id } => {
			WorkingTreeRequest::LocalCheckout {
				project_id: ProjectId(*project_id),
			}
		}
	}
}

fn workspace(workspace: &Workspace) -> wire::Workspace {
	let WorkspaceBase { selection, commit } = &workspace.base;
	wire::Workspace {
		workspace_id: workspace.workspace_id.0,
		conversation_id: workspace.conversation_id.0,
		project_id: workspace.project_id.0,
		root: workspace.root.display().to_string(),
		base: wire::WorkspaceBase {
			selection: match selection {
				BaseSelection::Head => wire::BaseSelection::Head,
				BaseSelection::Revision(revision) => {
					wire::BaseSelection::Revision {
						revision: revision.clone(),
					}
				}
			},
			commit: commit.clone(),
		},
		created_at_unix_ms: unix_ms(workspace.created_at),
	}
}

fn run(run: &Run) -> wire::Run {
	wire::Run {
		run_id: run.run_id.0,
		conversation_id: run.conversation_id.0,
		revision: run.revision.0,
		lifecycle: lifecycle(run.lifecycle),
		created_at_unix_ms: unix_ms(run.created_at),
		ended_at_unix_ms: run.ended_at.map(unix_ms),
	}
}

fn event_page(page: &EventPage) -> Result<wire::EventPage, CoreError> {
	Ok(wire::EventPage {
		cursor: page.cursor.0,
		events: page.events.iter().map(event).collect::<Result<_, _>>()?,
	})
}

fn event(event: &Event) -> Result<wire::Event, CoreError> {
	let EventPayload {
		kind,
		payload_version,
		payload,
	} = event.kind.encode()?;
	Ok(wire::Event {
		sequence: event.sequence.0,
		event_id: event.event_id.0,
		actor: actor(&event.actor),
		recorded_at_unix_ms: unix_ms(event.recorded_at),
		conversation_id: event.conversation_id.map(|id| id.0),
		run_id: event.run_id.map(|id| id.0),
		kind,
		payload_version,
		payload,
	})
}

pub(super) fn actor(actor: &Actor) -> wire::Actor {
	actor_of(actor.client_id())
}

/// The wire attribution of the Client identity an Actor acted through.
/// Every Actor this core knows is an interactive client, so this is the
/// one place that collapse is spelled.
pub(super) fn actor_of(client_id: ClientId) -> wire::Actor {
	wire::Actor::InteractiveClient {
		client_id: client_id.0,
	}
}

fn retention(retention: RetentionPolicy) -> wire::RetentionPolicy {
	match retention {
		RetentionPolicy::Retain => wire::RetentionPolicy::Retain,
		RetentionPolicy::ForgetAfterFinalRun => {
			wire::RetentionPolicy::ForgetAfterFinalRun
		}
	}
}

fn retention_from_wire(retention: wire::RetentionPolicy) -> RetentionPolicy {
	match retention {
		wire::RetentionPolicy::Retain => RetentionPolicy::Retain,
		wire::RetentionPolicy::ForgetAfterFinalRun => {
			RetentionPolicy::ForgetAfterFinalRun
		}
	}
}

fn lifecycle(lifecycle: RunLifecycle) -> wire::RunLifecycle {
	match lifecycle {
		RunLifecycle::Created => wire::RunLifecycle::Created,
		RunLifecycle::Starting => wire::RunLifecycle::Starting,
		RunLifecycle::Active => wire::RunLifecycle::Active,
		RunLifecycle::Stopping => wire::RunLifecycle::Stopping,
		RunLifecycle::Completed => wire::RunLifecycle::Completed,
		RunLifecycle::Failed => wire::RunLifecycle::Failed,
		RunLifecycle::Canceled => wire::RunLifecycle::Canceled,
		RunLifecycle::Lost => wire::RunLifecycle::Lost,
	}
}

fn lifecycle_from_wire(lifecycle: wire::RunLifecycle) -> RunLifecycle {
	match lifecycle {
		wire::RunLifecycle::Created => RunLifecycle::Created,
		wire::RunLifecycle::Starting => RunLifecycle::Starting,
		wire::RunLifecycle::Active => RunLifecycle::Active,
		wire::RunLifecycle::Stopping => RunLifecycle::Stopping,
		wire::RunLifecycle::Completed => RunLifecycle::Completed,
		wire::RunLifecycle::Failed => RunLifecycle::Failed,
		wire::RunLifecycle::Canceled => RunLifecycle::Canceled,
		wire::RunLifecycle::Lost => RunLifecycle::Lost,
	}
}

pub(crate) fn error(error: CoreError, minor: u32) -> wire::WireError {
	let restart = (minor >= wire::FENCED_READS_MINOR)
		.then(|| error.recovery_actions.iter().find_map(restart_metadata))
		.flatten();
	wire::WireError {
		category: category(error.category),
		code: error.code,
		retryable: error.retryable,
		message: error.message,
		revision_conflict: error.revision_conflict.map(revision_conflict),
		restart,
		recovery_actions: error
			.recovery_actions
			.into_iter()
			.filter_map(recovery_action)
			.collect(),
	}
}

fn recovery_action(action: RecoveryAction) -> Option<wire::RecoveryAction> {
	match action {
		RecoveryAction::RefreshRun { run_id } => {
			Some(wire::RecoveryAction::RefreshRun { run_id: run_id.0 })
		}
		RecoveryAction::RestartSnapshot { .. } => None,
	}
}

fn restart_metadata(action: &RecoveryAction) -> Option<wire::RestartMetadata> {
	match action {
		RecoveryAction::RefreshRun { .. } => None,
		RecoveryAction::RestartSnapshot { metadata } => Some(match metadata {
			jet_core::RestartMetadata::CursorExpired {
				minimum_available_cursor,
				current_snapshot_revision,
			} => wire::RestartMetadata::CursorExpired {
				minimum_available_cursor: minimum_available_cursor.0,
				current_snapshot_revision: current_snapshot_revision.0,
			},
			jet_core::RestartMetadata::CursorAhead {
				current_snapshot_revision,
			} => wire::RestartMetadata::CursorAhead {
				current_snapshot_revision: current_snapshot_revision.0,
			},
			jet_core::RestartMetadata::PaginationStale {
				current_snapshot_revision,
			} => wire::RestartMetadata::PaginationStale {
				current_snapshot_revision: current_snapshot_revision.0,
			},
		}),
	}
}

fn revision_conflict(conflict: RevisionConflict) -> wire::RevisionConflict {
	wire::RevisionConflict {
		current_revision: conflict.current_revision.0,
		safe_state: match conflict.safe_state {
			ConflictState::Run(current) => {
				wire::ConflictState::Run { run: run(&current) }
			}
		},
	}
}

fn category(category: ErrorCategory) -> wire::ErrorCategory {
	match category {
		ErrorCategory::InvalidInput => wire::ErrorCategory::InvalidInput,
		ErrorCategory::Unauthorized => wire::ErrorCategory::Unauthorized,
		ErrorCategory::Conflict => wire::ErrorCategory::Conflict,
		ErrorCategory::Unavailable => wire::ErrorCategory::Unavailable,
		ErrorCategory::Incompatible => wire::ErrorCategory::Incompatible,
		ErrorCategory::RateLimited => wire::ErrorCategory::RateLimited,
		ErrorCategory::NotFound => wire::ErrorCategory::NotFound,
		ErrorCategory::OutcomeUnknown => wire::ErrorCategory::OutcomeUnknown,
		ErrorCategory::Internal => wire::ErrorCategory::Internal,
	}
}

pub(super) fn unix_ms(time: SystemTime) -> i64 {
	match time.duration_since(UNIX_EPOCH) {
		Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
		Err(behind) => i64::try_from(behind.duration().as_millis())
			.map_or(i64::MIN, |ms| -ms),
	}
}

#[cfg(test)]
#[path = "minor_tests.rs"]
mod tests;
