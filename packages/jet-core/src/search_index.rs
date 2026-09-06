//! Keeping the Search index current (ADR-0036). The index is a projection
//! of committed semantic Events: after every Command and at every start,
//! the core reads the Events past the position the index has reached,
//! projects the human-visible content each one carries, and commits the
//! documents together with the new position. An interrupted indexer
//! resumes from that position; nothing it does touches the journal or
//! the Conversations it describes (ADR-0078).

use std::path::Path;

use jet_store::{
	NewSearchDocument, SEARCH_DOCUMENT_BODY_LIMIT, SEARCH_INDEX_BATCH_LIMIT,
	WriteTransaction,
};

use crate::Core;
use crate::conversation::ConversationId;
use crate::error::CoreError;
use crate::event::{Event, EventKind, EventSequence};
use crate::promotion::PromotionDestination;
use crate::search::SearchField;

impl Core {
	/// Brings the index up to the journal, one bounded batch per
	/// transaction. Each batch reads its position, its Events, and writes
	/// its documents behind the store's write lock, so two callers cannot
	/// index the same Event twice.
	pub(crate) async fn index_search(&self) -> Result<(), CoreError> {
		loop {
			let read =
				self.store.write(async |tx| index_batch(tx).await).await?;
			if read < SEARCH_INDEX_BATCH_LIMIT {
				return Ok(());
			}
		}
	}
}

/// Indexes one batch and returns how many Events it read.
async fn index_batch(tx: &mut WriteTransaction) -> Result<usize, CoreError> {
	let position = tx.search_index_position().await?;
	let records = tx
		.semantic_events_after(position, SEARCH_INDEX_BATCH_LIMIT)
		.await?;
	let Some(last) = records.last() else {
		return Ok(0);
	};
	let through_sequence = last.sequence;
	let read = records.len();
	let mut documents = Vec::new();
	for record in records {
		let event = Event::try_from(record)?;
		documents.extend(documents_of(&event));
	}
	tx.index_search_documents(documents, through_sequence)
		.await?;
	Ok(read)
}

/// The human-visible content one Event carries, as the documents to
/// index. Every kind decides for itself, so a new kind cannot reach the
/// index without choosing what a user may search for in it (ADR-0036).
pub(crate) fn documents_of(event: &Event) -> Vec<NewSearchDocument> {
	let Some(conversation_id) = event.conversation_id else {
		// Plane-level Events describe no Conversation.
		return Vec::new();
	};
	let documents = EventDocuments {
		conversation_id,
		sequence: event.sequence,
	};
	match &event.kind {
		EventKind::WorkspaceCreated { root, .. } => {
			vec![documents.path(root)]
		}
		EventKind::WorkspacePromotionRecorded { binding, .. } => {
			let branch = match &binding.destination {
				PromotionDestination::Branch(name) => {
					Some(documents.document(SearchField::Branch, name))
				}
				PromotionDestination::LocalCheckout => None,
			};
			branch
				.into_iter()
				.chain(binding.conflicts.iter().map(|conflict| {
					documents.document(SearchField::Path, &conflict.path)
				}))
				.collect()
		}
		// Identities, hashes, lifecycle states, and counts are not text a
		// user searches for.
		EventKind::ConversationCreated { .. }
		| EventKind::WorkspaceSeeded { .. }
		| EventKind::WorkspacePromotionSettled { .. }
		| EventKind::RunCreated {}
		| EventKind::RunLifecycleChanged { .. }
        | EventKind::RunActivityChanged { .. }
        | EventKind::RunProcessesChanged { .. }
        | EventKind::RunNativeConversation { .. }
        // Craft payloads stay opaque here; preserve the explicit search
        // allowlist rather than indexing native JSON or unknown views.
        | EventKind::RunOutput { .. }
		// Settings and Account bindings are Plane configuration, and a
		// binding sits next to a Credential; neither is Conversation
		// content (ADR-0076).
		| EventKind::SettingChanged { .. }
		| EventKind::SettingCleared { .. }
		| EventKind::AccountBound { .. }
		| EventKind::AccountUnbound { .. }
		// Security and Pairing Events are owner-only and secret-adjacent.
		| EventKind::AuditEpochBegun { .. }
		| EventKind::PairingGateChanged { .. }
		| EventKind::PairingOffered { .. }
		| EventKind::PairingClaimed { .. }
		| EventKind::PairingConfirmed { .. }
		| EventKind::PairingCompleted { .. }
		| EventKind::PairingOfferEnded { .. }
		| EventKind::PairedClientAccessChanged { .. }
		| EventKind::PairedClientRevoked { .. }
		| EventKind::ProjectRegistered { .. }
		// A kind this core cannot interpret is content it cannot vouch
		// for; a core that knows the kind indexes it when it rebuilds.
		| EventKind::Unrecognized(_) => Vec::new(),
	}
}

/// The documents one Event contributes, all sharing its reference.
struct EventDocuments {
	conversation_id: ConversationId,
	sequence: EventSequence,
}

impl EventDocuments {
	fn path(&self, path: &Path) -> NewSearchDocument {
		self.document(SearchField::Path, &path.to_string_lossy())
	}

	fn document(&self, field: SearchField, body: &str) -> NewSearchDocument {
		NewSearchDocument {
			conversation_id: self.conversation_id.0,
			sequence: self.sequence.0,
			field: field.as_str().into(),
			// A body past the store's bound is cut rather than refused: an
			// index that refused would stop advancing for every later
			// Event, and a path this long is not one a user types.
			body: body.chars().take(SEARCH_DOCUMENT_BODY_LIMIT).collect(),
		}
	}
}

#[cfg(test)]
#[path = "search_index_tests.rs"]
mod tests;
