use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{
	FixedDiscovery, actor, conversation_snapshot, events, register_repository,
	request, start_core_discovering,
};
use crate::{
	BaseSelection, ClientId, Command, CommandOutcome, Conversation,
	ConversationId, ConversationOrigin, Core, CoreError,
	DiscoveredConversation, ErrorCategory, EventKind, ExternalConversation,
	ExternalConversationList, ExternalOrigin, ExternalProcess, HarnessId,
	ImportId, ImportedConversation, NativeConversationId, ProjectId, Query,
	QueryResult, RetentionPolicy, SeedSelection, WorkingTree,
	WorkingTreeRequest,
};

fn codex() -> HarnessId {
	HarnessId("codex".into())
}

fn native(id: &str) -> NativeConversationId {
	NativeConversationId(id.into())
}

fn discovered(
	id: &str,
	working_directory: Option<&Path>,
	process: ExternalProcess,
) -> DiscoveredConversation {
	DiscoveredConversation {
		harness: codex(),
		native_conversation: native(id),
		working_directory: working_directory.map(Path::to_path_buf),
		process,
	}
}

async fn external_conversations(core: &Core) -> ExternalConversationList {
	let result = core
		.query(&actor(), Query::ExternalConversations)
		.await
		.unwrap();
	let QueryResult::ExternalConversations(list) = result else {
		panic!("unexpected result {result:?}");
	};
	list
}

async fn import(
	core: &Core,
	id: &str,
) -> Result<ImportedConversation, CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request(Command::ImportConversation {
				harness: codex(),
				native_conversation: native(id),
			}),
		)
		.await?;
	let CommandOutcome::ConversationImported(imported) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(imported)
}

async fn resume(
	core: &Core,
	import_id: ImportId,
	working_tree: WorkingTreeRequest,
) -> Result<Conversation, CoreError> {
	let outcome = core
		.execute(
			&actor(),
			request(Command::ResumeImportedConversation {
				import_id,
				retention: RetentionPolicy::Retain,
				working_tree,
			}),
		)
		.await?;
	let CommandOutcome::ConversationCreated(conversation) = outcome else {
		panic!("unexpected outcome {outcome:?}");
	};
	Ok(conversation)
}

fn refused(error: CoreError) -> (ErrorCategory, String) {
	(error.category, error.code)
}

/// Discovery is observed, not stored: every identity a supported Harness
/// reports is shown with the Project it falls in when one is registered,
/// with the directory it worked in otherwise, and with live takeover only
/// where the Harness advertises a cooperating endpoint. A PTY Jet does not
/// drive stays external (ADR-0010).
#[tokio::test]
async fn discovered_identities_show_their_project_and_what_jet_can_do() {
	let dir = tempfile::tempdir().unwrap();
	let discovery = FixedDiscovery::new(Vec::new());
	let core = start_core_discovering(
		&dir.path().join("plane.sqlite3"),
		discovery.clone(),
	)
	.await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	let repository = dir.path().join("repo").canonicalize().unwrap();
	let inside = repository.join("src");
	let elsewhere = dir.path().join("elsewhere");
	discovery.answer_with(vec![
		discovered(
			"thread-1",
			Some(&inside),
			ExternalProcess::Cooperating {
				pid: 41,
				endpoint: PathBuf::from("/run/user/1000/codex/41.sock"),
			},
		),
		discovered(
			"thread-2",
			Some(&elsewhere),
			ExternalProcess::External { pid: 42 },
		),
		discovered("thread-3", None, ExternalProcess::None),
	]);

	let before = external_conversations(&core).await;
	let imported = import(&core, "thread-2").await.unwrap();
	let after = external_conversations(&core).await;

	let expected = |import_id: Option<ImportId>| {
		vec![
			ExternalConversation {
				harness: codex(),
				native_conversation: native("thread-1"),
				origin: ExternalOrigin::Project {
					project_id,
					working_directory: inside.clone(),
				},
				process: ExternalProcess::Cooperating {
					pid: 41,
					endpoint: PathBuf::from("/run/user/1000/codex/41.sock"),
				},
				import_id: None,
			},
			ExternalConversation {
				harness: codex(),
				native_conversation: native("thread-2"),
				origin: ExternalOrigin::Unregistered {
					working_directory: elsewhere.clone(),
				},
				process: ExternalProcess::External { pid: 42 },
				import_id,
			},
			ExternalConversation {
				harness: codex(),
				native_conversation: native("thread-3"),
				origin: ExternalOrigin::Unknown,
				process: ExternalProcess::None,
				import_id: None,
			},
		]
	};
	assert_eq!(
		(
			before.discovered,
			before.imported,
			after.discovered,
			after.imported
		),
		(
			expected(None),
			Vec::new(),
			expected(Some(imported.import_id)),
			vec![imported],
		)
	);
}

/// An import registers what the Harness reported and nothing more: it is
/// not a Conversation, so no Run can be started from it, and an identity
/// the Plane cannot see is not registered at all (ADR-0010).
#[tokio::test]
async fn an_import_is_metadata_that_cannot_start_a_managed_run() {
	let dir = tempfile::tempdir().unwrap();
	let discovery = FixedDiscovery::new(vec![discovered(
		"thread-1",
		Some(Path::new("/home/jet/elsewhere")),
		ExternalProcess::None,
	)]);
	let core =
		start_core_discovering(&dir.path().join("plane.sqlite3"), discovery)
			.await;

	let imported = import(&core, "thread-1").await.unwrap();
	let again = import(&core, "thread-1").await.unwrap_err();
	let unseen = import(&core, "thread-9").await.unwrap_err();
	let malformed = import(&core, "thread\n1").await.unwrap_err();
	let run = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: ConversationId(imported.import_id.0),
			}),
		)
		.await
		.unwrap_err();
	let conversations =
		core.query(&actor(), Query::Conversations).await.unwrap();
	let QueryResult::Conversations(conversations) = conversations else {
		panic!("unexpected result {conversations:?}");
	};

	assert_eq!(
		(
			&imported,
			refused(again),
			refused(unseen),
			refused(malformed),
			refused(run),
			conversations.conversations,
			events(&core).await,
		),
		(
			&ImportedConversation {
				import_id: imported.import_id,
				harness: codex(),
				native_conversation: native("thread-1"),
				working_directory: Some(PathBuf::from("/home/jet/elsewhere")),
				imported_by: ClientId(Uuid::nil()),
				imported_at: imported.imported_at,
				resumed_as: None,
			},
			(ErrorCategory::Conflict, "import.already_imported".into()),
			(ErrorCategory::NotFound, "import.not_discovered".into()),
			(
				ErrorCategory::InvalidInput,
				"import.identity_invalid".into()
			),
			(ErrorCategory::NotFound, "conversation.not_found".into()),
			Vec::new(),
			vec![EventKind::ConversationImported {
				import_id: imported.import_id,
				harness: codex(),
				native_conversation: native("thread-1"),
				working_directory: Some(PathBuf::from("/home/jet/elsewhere")),
			}],
		)
	);
}

/// Managed Resume needs somewhere safe to work: the user registers or maps
/// a Project and picks a Workspace or its Local checkout, and the import is
/// continued by exactly one Conversation, which carries its origin and
/// admits managed Runs like any other (ADR-0010, ADR-0025).
#[tokio::test]
async fn resume_needs_a_project_and_continues_the_import_once() {
	let dir = tempfile::tempdir().unwrap();
	let discovery = FixedDiscovery::new(vec![
		discovered("thread-1", None, ExternalProcess::None),
		discovered("thread-2", None, ExternalProcess::None),
	]);
	let core =
		start_core_discovering(&dir.path().join("plane.sqlite3"), discovery)
			.await;
	let project_id = register_repository(&core, &dir.path().join("repo")).await;
	let first = import(&core, "thread-1").await.unwrap();
	let second = import(&core, "thread-2").await.unwrap();

	let unplaced =
		resume(&core, first.import_id, WorkingTreeRequest::NoProject)
			.await
			.unwrap_err();
	let unknown = resume(
		&core,
		ImportId(Uuid::nil()),
		WorkingTreeRequest::LocalCheckout { project_id },
	)
	.await
	.unwrap_err();
	let unregistered = resume(
		&core,
		first.import_id,
		WorkingTreeRequest::LocalCheckout {
			project_id: ProjectId(Uuid::nil()),
		},
	)
	.await
	.unwrap_err();
	let in_checkout = resume(
		&core,
		first.import_id,
		WorkingTreeRequest::LocalCheckout { project_id },
	)
	.await
	.unwrap();
	let twice = resume(
		&core,
		first.import_id,
		WorkingTreeRequest::LocalCheckout { project_id },
	)
	.await
	.unwrap_err();
	let in_workspace = resume(
		&core,
		second.import_id,
		WorkingTreeRequest::Workspace {
			project_id,
			base: BaseSelection::Head,
			seed: SeedSelection::None,
		},
	)
	.await
	.unwrap();
	let run = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: in_checkout.conversation_id,
			}),
		)
		.await;
	let workspace = conversation_snapshot(&core, in_workspace.conversation_id)
		.await
		.workspace;
	let imports = external_conversations(&core).await.imported;

	assert_eq!(
		(
			refused(unplaced),
			refused(unknown),
			refused(unregistered),
			refused(twice),
			in_checkout,
			in_workspace,
			run.map(|outcome| match outcome {
				CommandOutcome::RunCreated(run) => run.conversation_id,
				other => panic!("unexpected outcome {other:?}"),
			}),
			workspace.map(|workspace| workspace.conversation_id),
			imports,
			events(&core).await[3..5].to_vec(),
		),
		(
			(
				ErrorCategory::InvalidInput,
				"import.working_tree_required".into()
			),
			(ErrorCategory::NotFound, "import.not_found".into()),
			(ErrorCategory::NotFound, "project.not_found".into()),
			(ErrorCategory::Conflict, "import.already_resumed".into()),
			Conversation {
				conversation_id: in_checkout.conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTree::LocalCheckout { project_id },
				origin: ConversationOrigin::Imported {
					import_id: first.import_id,
				},
				created_at: in_checkout.created_at,
			},
			Conversation {
				conversation_id: in_workspace.conversation_id,
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTree::Workspace { project_id },
				origin: ConversationOrigin::Imported {
					import_id: second.import_id,
				},
				created_at: in_workspace.created_at,
			},
			Ok(in_checkout.conversation_id),
			Some(in_workspace.conversation_id),
			vec![
				ImportedConversation {
					resumed_as: Some(in_checkout.conversation_id),
					..first.clone()
				},
				ImportedConversation {
					resumed_as: Some(in_workspace.conversation_id),
					..second.clone()
				},
			],
			[
				EventKind::ConversationCreated {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::LocalCheckout { project_id },
					origin: ConversationOrigin::Imported {
						import_id: first.import_id,
					},
				},
				EventKind::ConversationCreated {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::Workspace { project_id },
					origin: ConversationOrigin::Imported {
						import_id: second.import_id,
					},
				},
			]
			.to_vec(),
		)
	);
}
