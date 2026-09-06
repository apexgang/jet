//! The external Conversation and import half of the translation seam
//! (ADR-0010, ADR-0049).

use jet_core::{
	ConversationOrigin, ExternalConversation, ExternalConversationList,
	ExternalOrigin, ExternalProcess, ImportedConversation,
};
use jet_protocol as wire;

use super::{actor_of, unix_ms};

pub(super) fn list(
	list: ExternalConversationList,
) -> wire::ExternalConversationList {
	wire::ExternalConversationList {
		cursor: list.cursor.0,
		discovered: list.discovered.into_iter().map(external).collect(),
		imported: list.imported.into_iter().map(imported).collect(),
	}
}

fn external(external: ExternalConversation) -> wire::ExternalConversation {
	wire::ExternalConversation {
		harness: external.harness.0,
		native_conversation: external.native_conversation.0,
		origin: match external.origin {
			ExternalOrigin::Project {
				project_id,
				working_directory,
			} => wire::ExternalOrigin::Project {
				project_id: project_id.0,
				working_directory: working_directory.display().to_string(),
			},
			ExternalOrigin::Unregistered { working_directory } => {
				wire::ExternalOrigin::Unregistered {
					working_directory: working_directory.display().to_string(),
				}
			}
			ExternalOrigin::Unknown => wire::ExternalOrigin::Unknown,
		},
		process: match external.process {
			ExternalProcess::None => wire::ExternalProcess::None,
			ExternalProcess::External { pid } => {
				wire::ExternalProcess::External { pid }
			}
			ExternalProcess::Cooperating { pid, endpoint } => {
				wire::ExternalProcess::Cooperating {
					pid,
					endpoint: endpoint.display().to_string(),
				}
			}
		},
		import_id: external.import_id.map(|import_id| import_id.0),
	}
}

pub(super) fn imported(
	imported: ImportedConversation,
) -> wire::ImportedConversation {
	wire::ImportedConversation {
		import_id: imported.import_id.0,
		harness: imported.harness.0,
		native_conversation: imported.native_conversation.0,
		working_directory: imported
			.working_directory
			.map(|directory| directory.display().to_string()),
		imported_by: actor_of(imported.imported_by),
		imported_at_unix_ms: unix_ms(imported.imported_at),
		resumed_as: imported
			.resumed_as
			.map(|conversation_id| conversation_id.0),
	}
}

pub(super) fn origin(origin: ConversationOrigin) -> wire::ConversationOrigin {
	match origin {
		ConversationOrigin::New => wire::ConversationOrigin::New,
		ConversationOrigin::Imported { import_id } => {
			wire::ConversationOrigin::Imported {
				import_id: import_id.0,
			}
		}
	}
}
