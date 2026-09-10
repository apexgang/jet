//! Query requests and replies at the domain-to-wire seam.

use super::{
	account, audit, auto_continue, capability, checkpoint, conversation_list,
	conversation_snapshot, craft_installation, extension, file_revision,
	file_target, file_target_from_wire, git_delivery, import, name, pairing,
	plane_status, project, promotion, run, schedule, search, setting, terminal,
	turn, usage, utility,
};
use jet_core::{
	AuditSequence, ClientId, ConversationId, CoreError, EventSequence,
	PathGrant, ProjectId, Query, QueryResult, RelativePath, RunId, SearchTerms,
	WorkspaceId,
};
use jet_protocol as wire;
use std::path::PathBuf;

/// The core form of a Query.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when a relative path is not
/// one the core accepts, so the core never receives an unvalidated path
/// (ADR-0101), or when a search text is empty or over its bounds.
pub(crate) fn query(
	request: &wire::QueryRequest,
	minor: u32,
) -> Result<Query, CoreError> {
	Ok(match request {
		wire::QueryRequest::RemoteToolReview {
			client_id,
			operation_id,
		} => Query::RemoteToolReview {
			client_id: ClientId(*client_id),
			operation_id: *operation_id,
		},
		wire::QueryRequest::InspectExtension {
			craft_id,
			extension_id,
		} => Query::InspectExtension {
			craft_id: craft_id.clone(),
			extension_id: extension_id.clone(),
		},
		wire::QueryRequest::ExtensionCatalog { craft_id } => {
			Query::ExtensionCatalog {
				craft_id: craft_id.clone(),
			}
		}
		wire::QueryRequest::ExtensionChange { change_id } => {
			Query::ExtensionChange {
				change_id: *change_id,
			}
		}
		wire::QueryRequest::DiscoverCraft { source } => Query::DiscoverCraft {
			source: craft_installation::source_from_wire(source),
		},
		wire::QueryRequest::EditableFile { target, path } => {
			Query::EditableFile {
				target: file_target_from_wire(*target),
				path: RelativePath::parse(path)?,
			}
		}
		wire::QueryRequest::WorkspaceTerminals { workspace_id } => {
			Query::WorkspaceTerminals {
				workspace_id: jet_core::WorkspaceId(*workspace_id),
			}
		}
		wire::QueryRequest::ChangeArtifact { sha256, offset } => {
			Query::ChangeArtifact {
				sha256: sha256.clone(),
				offset: *offset,
			}
		}
		wire::QueryRequest::ChangeDiff { run_id, scope } => Query::ChangeDiff {
			run_id: RunId(*run_id),
			scope: checkpoint::scope(scope),
		},
		wire::QueryRequest::NextChangeDiff { cursor } => {
			Query::NextChangeDiff {
				cursor: jet_core::PageCursor(cursor.0),
			}
		}
		wire::QueryRequest::OrphanedExecutions { after } => {
			Query::OrphanedExecutions {
				after: after.map(RunId),
			}
		}
		wire::QueryRequest::RunExecution { run_id } => Query::RunExecution {
			run_id: RunId(*run_id),
		},
		wire::QueryRequest::GitDeliveries { conversation_id } => {
			Query::GitDeliveries {
				conversation_id: ConversationId(*conversation_id),
			}
		}
		wire::QueryRequest::Utility { job_id } => {
			Query::Utility { job_id: *job_id }
		}
		wire::QueryRequest::AutoContinue { target } => Query::AutoContinue {
			target: auto_continue::target(*target),
		},
		wire::QueryRequest::ScheduledTasks { conversation_id } => {
			Query::ScheduledTasks {
				conversation_id: ConversationId(*conversation_id),
			}
		}
		wire::QueryRequest::TurnQueue { conversation_id } => Query::TurnQueue {
			conversation_id: ConversationId(*conversation_id),
		},
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
		wire::QueryRequest::Usage { selection } => Query::Usage {
			selection: usage::selection(*selection),
		},
		wire::QueryRequest::AccountBindings { observation } => {
			Query::AccountBindings {
				observation: capability::observation(*observation),
			}
		}
		wire::QueryRequest::Settings { scope, selection } => Query::Settings {
			scope: setting::scope_from_wire(*scope),
			selection: setting::selection_from_wire(*selection),
		},
		wire::QueryRequest::Events { after } => {
			if minor >= wire::NAMES_MINOR {
				Query::Events {
					after: EventSequence(*after),
				}
			} else {
				Query::LegacyEvents {
					after: EventSequence(*after),
				}
			}
		}
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
		wire::QueryRequest::PreviewPromotion {
			workspace_id,
			destination,
		} => Query::PreviewPromotion {
			workspace_id: WorkspaceId(*workspace_id),
			destination: promotion::destination_from_wire(destination),
		},
		wire::QueryRequest::Search { text } => {
			let terms = SearchTerms::parse(text)?;
			if minor >= wire::NAMES_MINOR {
				Query::Search { terms }
			} else {
				Query::LegacySearch { terms }
			}
		}
		wire::QueryRequest::ExternalConversations => {
			Query::ExternalConversations
		}
	})
}

pub(crate) fn query_result(
	result: QueryResult,
	minor: u32,
) -> Result<wire::QueryResponse, CoreError> {
	Ok(match result {
		QueryResult::RemoteToolReview(request) => {
			wire::QueryResponse::RemoteToolReview(crate::remote::tool::to_wire(
				request,
			))
		}
		QueryResult::ExtensionCatalog(catalog) => {
			wire::QueryResponse::ExtensionCatalog(extension::catalog(catalog))
		}
		QueryResult::ExtensionChange(change) => {
			wire::QueryResponse::ExtensionChange(extension::change(change))
		}
		QueryResult::CraftInstallationPreview(preview) => {
			wire::QueryResponse::CraftInstallationPreview(
				craft_installation::preview(*preview),
			)
		}
		QueryResult::EditableFile(file) => {
			wire::QueryResponse::EditableFile(wire::EditableFile {
				cursor: file.cursor.0,
				target: file_target(file.target),
				path: file.path.as_str().into(),
				revision: file_revision(file.revision),
				content: file.content,
			})
		}
		QueryResult::WorkspaceTerminals { cursor, terminals } => {
			wire::QueryResponse::WorkspaceTerminals {
				cursor: cursor.0,
				terminals: terminals
					.into_iter()
					.map(terminal::snapshot)
					.collect(),
			}
		}
		QueryResult::ChangeArtifact(chunk) => {
			wire::QueryResponse::ChangeArtifact(wire::ChangeArtifactChunk {
				artifact: checkpoint::artifact(chunk.artifact, minor),
				offset: chunk.offset,
				bytes: chunk.bytes,
			})
		}
		QueryResult::ChangeDiff(diff) => wire::QueryResponse::ChangeDiff(
			Box::new(checkpoint::diff(*diff, minor)),
		),
		QueryResult::GitDeliveries(deliveries) => {
			wire::QueryResponse::GitDeliveries {
				deliveries: deliveries
					.into_iter()
					.map(git_delivery::delivery)
					.collect(),
			}
		}
		QueryResult::Utility(job) => {
			wire::QueryResponse::Utility(utility::job(job))
		}
		QueryResult::AutoContinue(value) => {
			wire::QueryResponse::AutoContinue(auto_continue::snapshot(*value))
		}
		QueryResult::ScheduledTasks(snapshot) => {
			wire::QueryResponse::ScheduledTasks(wire::ScheduledTasks {
				cursor: snapshot.cursor.0,
				tasks: snapshot.tasks.into_iter().map(schedule::task).collect(),
			})
		}
		QueryResult::TurnQueue(queue) => {
			wire::QueryResponse::TurnQueue(wire::TurnQueue {
				cursor: queue.cursor.0,
				turns: queue.turns.into_iter().map(turn::turn).collect(),
			})
		}
		QueryResult::OrphanedExecutions(page) => {
			wire::QueryResponse::OrphanedExecutions(run::orphans(page, minor))
		}
		QueryResult::RunExecution(execution) => {
			wire::QueryResponse::RunExecution(run::execution(execution, minor))
		}
		QueryResult::Status(status) => {
			wire::QueryResponse::Status(plane_status(&status, minor))
		}
		QueryResult::Conversations(list) => {
			wire::QueryResponse::Conversations(conversation_list(&list, minor))
		}
		QueryResult::Conversation(snapshot) => {
			wire::QueryResponse::Conversation(Box::new(conversation_snapshot(
				&snapshot, minor,
			)))
		}
		QueryResult::Capabilities(snapshot) => {
			wire::QueryResponse::Capabilities(capability::snapshot(
				snapshot, minor,
			))
		}
		QueryResult::AccountBindings(bindings) => {
			wire::QueryResponse::AccountBindings(account::list(bindings))
		}
		QueryResult::Usage(snapshot) => {
			wire::QueryResponse::Usage(Box::new(usage::plane(*snapshot)))
		}
		QueryResult::Settings(snapshot) => {
			wire::QueryResponse::Settings(setting::snapshot(snapshot, minor))
		}
		QueryResult::Events(page) => {
			wire::QueryResponse::Events(name::event_page(&page, minor)?)
		}
		QueryResult::Pairing(snapshot) => {
			wire::QueryResponse::Pairing(pairing::snapshot(snapshot))
		}
		QueryResult::SecurityAudit(page) => {
			wire::QueryResponse::SecurityAudit(audit::page(page, minor)?)
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
		QueryResult::PromotionPreview(preview) => {
			wire::QueryResponse::PromotionPreview(Box::new(promotion::preview(
				*preview,
			)))
		}
		QueryResult::Search(result) => {
			wire::QueryResponse::Search(search::result(result, minor))
		}
		QueryResult::ExternalConversations(list) => {
			wire::QueryResponse::ExternalConversations(import::list(list))
		}
	})
}
