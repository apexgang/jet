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

fn event(conversation_id: Option<ConversationId>, kind: EventKind) -> Event {
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
		destination_commit: "0123456789abcdef0123456789abcdef01234567".into(),
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
