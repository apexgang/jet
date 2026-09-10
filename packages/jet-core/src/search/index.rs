//! Keeping the Search index current (ADR-0036). The index is a projection
//! of committed semantic Events: after every Command and at every start,
//! the core reads the Events past the position the index has reached,
//! projects the human-visible content each one carries, and commits the
//! documents together with the new position. An interrupted indexer
//! resumes from that position; nothing it does touches the journal or
//! the Conversations it describes (ADR-0078).

use crate::{
	Core,
	conversation::ConversationId,
	error::CoreError,
	event::{Event, EventKind, EventSequence},
	promotion::PromotionDestination,
	search::SearchField,
};
use jet_store::{
	NewSearchDocument, SEARCH_DOCUMENT_BODY_LIMIT, SEARCH_INDEX_BATCH_LIMIT,
	WriteTransaction,
};
use std::path::Path;

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

	/// Finishes the independently watermarked historical name repair before a
	/// name-aware Search claims a complete result. Startup and ordinary Commands
	/// do only one batch; an explicit Search may drain the resumable backlog.
	pub(crate) async fn index_search_names(&self) -> Result<(), CoreError> {
		loop {
			let read = self
				.store
				.write(async |tx| tx.reconcile_name_documents().await)
				.await?;
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
	// Historical name repair is deliberately one bounded batch per indexing
	// attempt. It must not make startup traverse an old Plane's whole journal;
	// later Commands resume it, while a name-aware Search explicitly drains it.
	tx.reconcile_name_documents().await?;
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
		EventKind::ConversationNameChanged { name }
		| EventKind::RunNameChanged { name } => {
			vec![documents.document(SearchField::Name, &name.value)]
		}
		EventKind::ConversationCreated {
			name: Some(name), ..
		}
		| EventKind::RunCreated { name: Some(name) } => {
			vec![documents.document(SearchField::Name, &name.value)]
		}
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
		EventKind::HandoffCreated { .. }
		| EventKind::UserEditApplied { .. }
		| EventKind::ReviewSubmitted { .. }
		| EventKind::ConversationCreated { name: None, .. }
		| EventKind::RunCreated { name: None }
		| EventKind::ChangeEvidenceRecorded { .. }
		| EventKind::ChangeCheckpointRecorded { .. }
        | EventKind::ArtifactPublished { .. }
		| EventKind::TurnInput { .. }
		| EventKind::AutoContinueChanged { .. }
		| EventKind::AutoContinueConfigured { .. }
		| EventKind::ScheduleCreated { .. } | EventKind::ScheduleCanceled { .. } | EventKind::ScheduleFired { .. } | EventKind::TurnChanged { .. }
		| EventKind::RunControlRequested { .. }
		| EventKind::RunTerminated { .. }
		| EventKind::UsageRecorded { .. }
		| EventKind::ConversationImported { .. }
		| EventKind::WorkspaceSeeded { .. }
		| EventKind::WorkspacePromotionSettled { .. }
		| EventKind::TerminalStateChanged { .. }
		| EventKind::RunLifecycleChanged { .. }
        | EventKind::RunActivityChanged { .. }
        | EventKind::RunProcessesChanged { .. }
        | EventKind::RunNativeConversation { .. }
        // Craft payloads stay opaque here; preserve the explicit search
        // allowlist rather than indexing native JSON or unknown views.
        | EventKind::RunOutput { .. }
        // An approval request and its review are a security decision about
        // Conversation content, not text a user looks for by name.
        | EventKind::ApprovalRequested { .. }
        | EventKind::ApprovalRetryAuthorized { .. }
        | EventKind::ApprovalReviewed { .. }
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
mod tests {
	use std::time::SystemTime;

	use jet_store::NewSearchDocument;
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::documents_of;
	use crate::test_support::actor;
	use crate::{
		ConflictKind, ConversationId, Event, EventId, EventKind, EventSequence,
		PromotionBinding, PromotionConflict, PromotionDestination, PromotionId,
		PromotionState, WorkspaceId, WorkspaceSeed,
	};

	fn event(
		conversation_id: Option<ConversationId>,
		kind: EventKind,
	) -> Event {
		Event {
			sequence: EventSequence(7),
			event_id: EventId(Uuid::nil()),
			actor: actor().into(),
			recorded_at: SystemTime::UNIX_EPOCH,
			conversation_id,
			run_id: None,
			kind,
		}
	}

	fn binding(
		destination: PromotionDestination,
		conflicts: Vec<PromotionConflict>,
	) -> PromotionBinding {
		PromotionBinding {
			workspace_id: WorkspaceId(Uuid::nil()),
			destination,
			base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
			workspace_tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
			destination_commit: "0123456789abcdef0123456789abcdef01234567"
				.into(),
			destination_tree: "fedcba9876543210fedcba9876543210fedcba98".into(),
			result_tree: "0fedcba9876543210fedcba9876543210fedcba9".into(),
			destination_dirty: false,
			conflicts,
			actor: actor().client_id(),
		}
	}

	/// A promotion contributes the branch it targets and every path it could
	/// not settle; hashes, identities, and flags stay out (ADR-0036).
	#[test]
	fn a_promotion_indexes_its_branch_and_unsettled_paths() {
		let conversation_id = ConversationId(Uuid::now_v7());
		let recorded = event(
			Some(conversation_id),
			EventKind::WorkspacePromotionRecorded {
				workspace_id: WorkspaceId(Uuid::nil()),
				promotion_id: PromotionId(Uuid::nil()),
				binding: binding(
					PromotionDestination::Branch("feature/search".into()),
					vec![
						PromotionConflict {
							path: "src/lib.rs".into(),
							kind: ConflictKind::Diverged,
						},
						PromotionConflict {
							path: "docs/index.md".into(),
							kind: ConflictKind::Diverged,
						},
					],
				),
				state: PromotionState::Conflicted,
			},
		);

		assert_eq!(
			documents_of(&recorded),
			vec![
				NewSearchDocument {
					conversation_id: conversation_id.0,
					sequence: 7,
					field: "branch".into(),
					body: "feature/search".into(),
				},
				NewSearchDocument {
					conversation_id: conversation_id.0,
					sequence: 7,
					field: "path".into(),
					body: "src/lib.rs".into(),
				},
				NewSearchDocument {
					conversation_id: conversation_id.0,
					sequence: 7,
					field: "path".into(),
					body: "docs/index.md".into(),
				},
			]
		);
	}

	/// A Conversation Event whose payload is hashes and counts contributes
	/// nothing, and neither does content that belongs to no Conversation.
	#[test]
	fn hashes_counts_and_plane_level_content_index_nothing() {
		let seeded = event(
			Some(ConversationId(Uuid::now_v7())),
			EventKind::WorkspaceSeeded {
				workspace_id: WorkspaceId(Uuid::nil()),
				seed: WorkspaceSeed {
					tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
					changed_paths: 3,
				},
			},
		);
		let registered = event(
			None,
			EventKind::ProjectRegistered {
				project_id: crate::ProjectId(Uuid::nil()),
				root: "/home/user/project".into(),
			},
		);

		assert_eq!(
			(documents_of(&seeded), documents_of(&registered)),
			(Vec::new(), Vec::new())
		);
	}
}
