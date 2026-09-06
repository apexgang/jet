use jet_store::Store;
use pretty_assertions::assert_eq;

use crate::event::EventSubject;
use crate::test_support::{
	Diverged, actor, diverged, preview_promotion, request, start_core,
};
use crate::{
	Command, CommandOutcome, Core, CoreError, CredentialSource, EventKind,
	EventSequence, PairingGate, PairingMethod, PromotionDestination,
	ProviderId, Query, QueryResult, SearchField, SearchHit, SearchResult,
	SearchTerms,
};

async fn search(core: &Core, text: &str) -> SearchResult {
	let result = core
		.query(
			&actor(),
			Query::Search {
				terms: SearchTerms::parse(text).unwrap(),
			},
		)
		.await
		.unwrap();
	let QueryResult::Search(found) = result else {
		panic!("unexpected result {result:?}");
	};
	found
}

/// The sequence of the newest journal Event of `kind`, oldest first.
async fn sequence_of(core: &Core, kind: &str) -> EventSequence {
	let result = core
		.query(
			&actor(),
			Query::Events {
				after: EventSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::Events(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.events
		.into_iter()
		.rev()
		.find(|event| event.kind.encode().unwrap().kind == kind)
		.map(|event| event.sequence)
		.unwrap()
}

/// A hit names the Conversation and the Event that carried the content:
/// the Workspace root from the Event that created it, an unsettled path
/// from the Event that recorded the promotion (ADR-0036).
#[tokio::test]
async fn hits_reference_the_conversation_and_the_event_that_carried_them() {
	let dir = tempfile::tempdir().unwrap();
	let Diverged {
		core,
		repository,
		workspace,
		..
	} = diverged(dir.path()).await;
	std::fs::write(repository.join("f.txt"), "X\nb\nC\n").unwrap();
	let preview = preview_promotion(
		&core,
		workspace.workspace_id,
		PromotionDestination::LocalCheckout,
	)
	.await
	.unwrap();
	let outcome = core
		.execute(
			&actor(),
			request(Command::PromoteWorkspace {
				binding: preview.binding,
			}),
		)
		.await
		.unwrap();
	assert!(matches!(
		outcome,
		CommandOutcome::WorkspacePromotionRecorded(_)
	));

	let conflicted = search(&core, "f.txt").await;
	let created = search(&core, "workspaces").await;

	assert_eq!(
		(&conflicted, &created),
		(
			&SearchResult {
				cursor: conflicted.cursor,
				indexed_through: conflicted.cursor,
				hits: vec![SearchHit {
					conversation_id: workspace.conversation_id,
					sequence: sequence_of(
						&core,
						"workspace.promotion_recorded"
					)
					.await,
					field: SearchField::Path,
					excerpt: "f.txt".into(),
				}],
			},
			&SearchResult {
				cursor: conflicted.cursor,
				indexed_through: conflicted.cursor,
				hits: vec![SearchHit {
					conversation_id: workspace.conversation_id,
					sequence: sequence_of(&core, "workspace.created").await,
					field: SearchField::Path,
					excerpt: workspace.root.to_string_lossy().into_owned(),
				}],
			}
		)
	);
}

/// What sits beside a Credential or a Pairing secret is never indexed,
/// however visible its label is elsewhere (ADR-0036, ADR-0076).
#[tokio::test]
async fn account_labels_and_pairing_are_not_searchable() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("plane.sqlite3")).await;
	core.execute(
		&actor(),
		request(Command::BindAccount {
			provider: ProviderId("openai".into()),
			label: "Acme billing".into(),
			provider_account: None,
			credential_source: CredentialSource::PlatformStore,
		}),
	)
	.await
	.unwrap();
	core.execute(
		&actor(),
		request(Command::SetPairingGate {
			gate: PairingGate::Open,
		}),
	)
	.await
	.unwrap();
	core.execute(
		&actor(),
		request(Command::OpenPairing {
			method: PairingMethod::ManualCode,
		}),
	)
	.await
	.unwrap();

	let found = search(&core, "acme").await;

	assert_eq!(
		found,
		SearchResult {
			cursor: found.cursor,
			indexed_through: found.cursor,
			hits: vec![],
		}
	);
	assert!(found.cursor > EventSequence(0));
}

/// A daemon that stopped between a Command's commit and its indexing
/// picks up from the position the index reached, so nothing is missed
/// and nothing is indexed twice, and the journal is only read (ADR-0078).
#[tokio::test]
async fn indexing_resumes_from_where_it_stopped() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let Diverged {
		core, workspace, ..
	} = diverged(dir.path()).await;
	let before = search(&core, "workspaces").await;
	core.close().await;
	drop(core);
	// An Event committed by a Command whose indexing never ran.
	let store = Store::open(&path).await.unwrap();
	let unindexed = store
		.write(async |tx| {
			let event = EventKind::WorkspaceCreated {
				workspace_id: workspace.workspace_id,
				project_id: workspace.project_id,
				root: "/elsewhere/workspaces/interrupted".into(),
				base: workspace.base.clone(),
			}
			.to_record(
				&actor(),
				EventSubject::Conversation(workspace.conversation_id),
				0,
			)?;
			Ok::<_, CoreError>(tx.append_event(event).await?.sequence)
		})
		.await
		.unwrap();
	store.close().await;
	drop(store);

	let core = start_core(&path).await;
	let after = search(&core, "workspaces").await;

	assert_eq!(
		after,
		SearchResult {
			cursor: EventSequence(unindexed),
			indexed_through: EventSequence(unindexed),
			hits: vec![
				SearchHit {
					conversation_id: workspace.conversation_id,
					sequence: EventSequence(unindexed),
					field: SearchField::Path,
					excerpt: "/elsewhere/workspaces/interrupted".into(),
				},
				before.hits[0].clone(),
			],
		}
	);
}

/// Search text is bounded before it reaches the index (ASVS 2.2.1).
#[test]
fn search_text_is_bounded() {
	let empty = SearchTerms::parse("  \t ").unwrap_err();
	let long = SearchTerms::parse(&"a".repeat(257)).unwrap_err();
	let many = SearchTerms::parse(&"a ".repeat(17)).unwrap_err();
	let fine = SearchTerms::parse(" src/lib.rs  search ").unwrap();

	assert_eq!(
		(empty.code, long.code, many.code, fine.as_slice()),
		(
			"search.empty".into(),
			"search.text_too_long".into(),
			"search.too_many_terms".into(),
			&["src/lib.rs".to_owned(), "search".to_owned()][..]
		)
	);
}
