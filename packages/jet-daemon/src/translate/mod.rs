//! Translation between core domain types and versioned wire types
//! (ADR-0049). This is the only place the two vocabularies meet; its
//! Account binding, Capability, and Setting parts sit in the submodules
//! beside it.

mod account;
mod audit;
mod capability;
mod setting;

pub(crate) use capability::snapshot as capabilities;

use std::time::{SystemTime, UNIX_EPOCH};

use jet_core::{
	AccountBindingId, Actor, AuditSequence, Command, CommandOutcome,
	ConflictState, Conversation, ConversationId, ConversationList,
	ConversationSnapshot, CoreError, ErrorCategory, Event, EventPage,
	EventPayload, EventSequence, PlaneStatus, ProviderId, Query, QueryResult,
	RecoveryAction, RetentionPolicy, Revision, RevisionConflict, Run, RunId,
	RunLifecycle,
};
use jet_protocol as wire;

pub(crate) fn query(request: &wire::QueryRequest, minor: u32) -> Query {
	match request {
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
		wire::QueryRequest::SecurityAudit { after } => Query::SecurityAudit {
			after: AuditSequence(*after),
		},
	}
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
			wire::QueryResponse::Conversations(conversation_list(&list))
		}
		QueryResult::Conversation(snapshot) => {
			wire::QueryResponse::Conversation(conversation_snapshot(&snapshot))
		}
		QueryResult::Capabilities(snapshot) => {
			wire::QueryResponse::Capabilities(capability::snapshot(snapshot))
		}
		QueryResult::AccountBindings(bindings) => {
			wire::QueryResponse::AccountBindings(account::list(bindings))
		}
		QueryResult::Settings(snapshot) => {
			wire::QueryResponse::Settings(setting::snapshot(snapshot))
		}
		QueryResult::Events(page) => {
			wire::QueryResponse::Events(event_page(&page)?)
		}
		QueryResult::SecurityAudit(page) => {
			wire::QueryResponse::SecurityAudit(audit::page(page))
		}
	})
}

pub(crate) fn command(request: &wire::CommandRequest) -> Command {
	match request {
		wire::CommandRequest::CreateConversation { retention } => {
			Command::CreateConversation {
				retention: retention_from_wire(*retention),
			}
		}
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
		wire::CommandRequest::TransitionRun {
			run_id,
			expected_revision,
			lifecycle,
		} => Command::TransitionRun {
			run_id: RunId(*run_id),
			expected_revision: Revision(*expected_revision),
			lifecycle: lifecycle_from_wire(*lifecycle),
		},
	}
}

pub(crate) fn command_outcome(
	outcome: CommandOutcome,
) -> wire::CommandResponse {
	match outcome {
		CommandOutcome::ConversationCreated(created) => {
			wire::CommandResponse::ConversationCreated(conversation(&created))
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
	}
}

fn plane_status(status: &PlaneStatus, minor: u32) -> wire::PlaneStatus {
	wire::PlaneStatus {
		cursor: (minor >= wire::FENCED_READS_MINOR).then_some(status.cursor.0),
		plane_id: status.plane_id.0,
		daemon_starts: status.daemon_starts,
		started_at_unix_ms: unix_ms(status.started_at),
		core_version: status.core_version.into(),
	}
}

fn conversation_list(list: &ConversationList) -> wire::ConversationList {
	wire::ConversationList {
		cursor: list.cursor.0,
		conversations: list.conversations.iter().map(conversation).collect(),
		next_page: list
			.next_page
			.as_ref()
			.map(|cursor| wire::PageCursor(cursor.0)),
	}
}

fn conversation_snapshot(
	snapshot: &ConversationSnapshot,
) -> wire::ConversationSnapshot {
	wire::ConversationSnapshot {
		cursor: snapshot.cursor.0,
		conversation: conversation(&snapshot.conversation),
		runs: snapshot.runs.iter().map(run).collect(),
	}
}

fn conversation(conversation: &Conversation) -> wire::Conversation {
	wire::Conversation {
		conversation_id: conversation.conversation_id.0,
		retention: retention(conversation.retention),
		created_at_unix_ms: unix_ms(conversation.created_at),
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
	match actor {
		Actor::InteractiveClient { client_id } => {
			wire::Actor::InteractiveClient {
				client_id: client_id.0,
			}
		}
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
