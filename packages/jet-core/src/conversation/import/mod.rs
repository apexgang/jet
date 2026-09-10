//! Imported conversations: Harness-native Conversation identities
//! discovered outside Jet and registered so a managed Run can continue
//! them (ADR-0010).
//!
//! An import is metadata. It records the identity as the Harness spells
//! it and the directory the Harness reported working in; it is not a
//! Conversation, has no working tree, and starts no Run. Managed Resume
//! turns it into a Conversation the way every managed Conversation is
//! made: in a Workspace of a registered Project, or explicitly in that
//! Project's Local checkout (ADR-0025). Whether the Project is the one the
//! Harness worked in is the user's to decide; Jet only insists there is
//! one. Jet never seizes the process that holds an external Conversation:
//! live takeover is reported only where the Harness advertises a
//! cooperating structured endpoint, and a PTY stays external.

mod discovery;
pub(crate) use discovery::external_conversations;

use crate::{
	Actor, ClientId, Core, ProjectId,
	capability::HarnessId,
	command::{CommandOutcome, preparation::Prepared},
	conversation::{ConversationId, ConversationOrigin},
	error::CoreError,
	event::{EventKind, EventSequence, EventSubject},
	system_time,
	workspace::{self, WorkingTreeRequest, WorkspaceHome},
};
use jet_store::{
	ImportedConversationRecord, NewImportedConversation, RetentionPolicy,
	WriteTransaction,
};
use serde::{Deserialize, Serialize};
use std::{
	path::{Path, PathBuf},
	time::SystemTime,
};
use uuid::Uuid;

/// Longest Harness or native identity the core accepts, as the store bounds
/// them. A native identity is a UUID or a short token; the bound keeps a
/// hostile client from storing a novel.
const MAX_HARNESS_CHARS: usize = 128;
const MAX_IDENTITY_CHARS: usize = 1024;
/// Longest working directory the core records, as the store bounds it.
const MAX_DIRECTORY_CHARS: usize = 4096;

/// Durable identity of one Imported conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImportId(pub Uuid);

/// A Conversation identity as its Harness spells it, such as a Codex thread
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeConversationId(pub String);

/// A process outside Jet's management that holds an external
/// Conversation live, and what Jet can do about it (see `External
/// process`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalProcess {
	/// No live process was observed. The identity can be continued only by
	/// a new managed Run.
	None,
	/// A live process Jet can see only through a terminal. It stays
	/// external: Jet does not seize a PTY it does not drive.
	External {
		/// The process as the operating system numbers it.
		pid: u32,
	},
	/// A live process whose Harness advertises a cooperating structured
	/// endpoint, so live takeover is available there.
	Cooperating {
		/// The process as the operating system numbers it.
		pid: u32,
		/// The endpoint the Harness advertises.
		endpoint: PathBuf,
	},
}

/// One Harness-native Conversation as a discovery observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConversation {
	/// The Harness whose identity it is.
	pub harness: HarnessId,
	/// The identity as the Harness spells it.
	pub native_conversation: NativeConversationId,
	/// The directory the Harness reported working in, if it reported one.
	pub working_directory: Option<PathBuf>,
	/// The live process holding it, if any.
	pub process: ExternalProcess,
}

/// Where an external Conversation did its work, as it relates to this
/// Plane's Projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalOrigin {
	/// Inside a registered Project, which a Resume may select directly.
	Project {
		/// The Project whose root holds the directory.
		project_id: ProjectId,
		/// The directory the Harness reported.
		working_directory: PathBuf,
	},
	/// In a directory no Project covers. The user registers it, or maps
	/// another Project, before a Resume.
	Unregistered {
		/// The directory the Harness reported.
		working_directory: PathBuf,
	},
	/// The Harness did not say where it worked.
	Unknown,
}

/// One external Conversation as the Plane presents it: what was observed,
/// placed against the Projects and imports the Plane has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalConversation {
	/// The Harness whose identity it is.
	pub harness: HarnessId,
	/// The identity as the Harness spells it.
	pub native_conversation: NativeConversationId,
	/// Where it did its work.
	pub origin: ExternalOrigin,
	/// The live process holding it, if any.
	pub process: ExternalProcess,
	/// The import that already registered it, if one has.
	pub import_id: Option<ImportId>,
}

/// One Imported conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedConversation {
	/// Durable identity.
	pub import_id: ImportId,
	/// The Harness whose identity it is.
	pub harness: HarnessId,
	/// The identity as the Harness spells it.
	pub native_conversation: NativeConversationId,
	/// The directory the Harness reported working in when it was imported.
	pub working_directory: Option<PathBuf>,
	/// The Client identity of the interactive user who imported it.
	pub imported_by: ClientId,
	/// When it was imported.
	pub imported_at: SystemTime,
	/// The Conversation that continues it, once a Resume has made one.
	pub resumed_as: Option<ConversationId>,
}

/// The external Conversations the Plane can see and the imports it holds,
/// fenced by the journal position the imports were read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalConversationList {
	/// Newest Event sequence visible when the imports were read.
	pub cursor: EventSequence,
	/// Every identity the Plane can see right now, as discovery reported
	/// them.
	pub discovered: Vec<ExternalConversation>,
	/// Every import the Plane holds, in the order they were made.
	pub imported: Vec<ImportedConversation>,
}

impl NativeConversationId {
	/// Refuses an identity the store cannot keep or a Harness could not
	/// have spelled, before it reaches a discovery comparison or a row.
	fn validate(&self) -> Result<(), CoreError> {
		validate_token(&self.0, MAX_IDENTITY_CHARS)
	}
}

fn validate_token(token: &str, max_chars: usize) -> Result<(), CoreError> {
	let malformed = token.is_empty()
		|| token.chars().count() > max_chars
		|| token.chars().any(char::is_control);
	if malformed {
		return Err(CoreError::invalid_input(
			"import.identity_invalid",
			"a Harness and its native Conversation identity are non-empty, \
			 bounded, and free of control characters",
		));
	}
	Ok(())
}

/// Finds the identity an import names among what the Plane can see right
/// now, before the transaction opens: a refusal that describes the machine
/// leaves no receipt behind (ADR-0093), and an identity the Plane cannot
/// see is not registered on a client's say-so.
///
/// # Errors
///
/// Returns `import.identity_invalid` for an identity the core will not
/// keep, and a `not_found` `import.not_discovered` when no supported
/// Harness reports it.
pub(crate) async fn prepare_import(
	core: &Core,
	harness: &HarnessId,
	native_conversation: &NativeConversationId,
) -> Result<DiscoveredConversation, CoreError> {
	validate_token(&harness.0, MAX_HARNESS_CHARS)?;
	native_conversation.validate()?;
	core.discovery
		.discover()
		.await
		.into_iter()
		.find(|found| {
			found.harness == *harness
				&& found.native_conversation == *native_conversation
		})
		.ok_or_else(|| {
			CoreError::not_found(
				"import.not_discovered",
				"no supported Harness on this Plane reports that Conversation; \
				 Jet registers only the identities it can see",
			)
		})
}

/// Records the identity discovery found as an import and journals it.
///
/// # Errors
///
/// Returns a `conflict` `import.already_imported` when the identity is
/// registered already, or a store category when the row cannot be written.
pub(crate) async fn import(
	tx: &mut WriteTransaction,
	actor: &Actor,
	prepared: Prepared,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Prepared::Import(found) = prepared else {
		return Err(CoreError::internal(
			"import.unprepared",
			"an import reached its transaction without the identity discovery \
			 found",
		));
	};
	let DiscoveredConversation {
		harness,
		native_conversation,
		working_directory,
		process: _,
	} = found;
	if tx
		.imported_conversation_by_identity(&harness.0, &native_conversation.0)
		.await?
		.is_some()
	{
		return Err(CoreError::conflict(
			"import.already_imported",
			"that Conversation is already imported",
		));
	}
	let working_directory_text = match &working_directory {
		Some(directory) => Some(directory_text(directory)?),
		None => None,
	};
	let imported: ImportedConversation = tx
		.insert_imported_conversation(NewImportedConversation {
			import_id: Uuid::now_v7(),
			harness: harness.0.clone(),
			native_conversation: native_conversation.0.clone(),
			working_directory: working_directory_text,
			imported_by: actor.record(),
			imported_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationImported {
		import_id: imported.import_id,
		harness,
		native_conversation,
		working_directory,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Plane,
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::ConversationImported(imported))
}

/// The directory as the store keeps it. A discovery reports what the
/// Harness said; one that cannot be recorded is the Plane's failing, not
/// the client's.
fn directory_text(directory: &Path) -> Result<String, CoreError> {
	let text = directory.to_str().ok_or_else(|| {
		CoreError::internal(
			"import.working_directory_not_unicode",
			"a discovery reported a working directory that is not Unicode",
		)
	})?;
	if text.chars().count() > MAX_DIRECTORY_CHARS {
		return Err(CoreError::internal(
			"import.working_directory_too_long",
			"a discovery reported a working directory longer than Jet records",
		));
	}
	Ok(text.to_owned())
}

/// Refuses a Resume that names nowhere to work, before anything is
/// prepared: an import continues only in a Workspace or a Local checkout
/// of a registered Project (ADR-0010, ADR-0025).
///
/// # Errors
///
/// Returns `import.working_tree_required`.
pub(crate) fn require_working_tree(
	working_tree: &WorkingTreeRequest,
) -> Result<(), CoreError> {
	match working_tree {
		WorkingTreeRequest::NoProject => Err(CoreError::invalid_input(
			"import.working_tree_required",
			"an Imported conversation is continued in a Workspace or the Local \
			 checkout of a registered Project; register or map one first",
		)),
		WorkingTreeRequest::Workspace { .. }
		| WorkingTreeRequest::LocalCheckout { .. } => Ok(()),
	}
}

/// A managed Resume as it reaches its transaction.
pub(crate) struct Resume {
	/// The import to continue.
	pub(crate) import_id: ImportId,
	/// Whether Jet keeps the Conversation after its final Run.
	pub(crate) retention: RetentionPolicy,
	/// Where it does its work, already known to name a Project.
	pub(crate) working_tree: WorkingTreeRequest,
	/// What its preparation produced: a Workspace when it asked for one.
	pub(crate) prepared: Prepared,
}

/// Continues an import as a new Conversation where the Resume asks: the
/// import exists, no Conversation continues it yet, and the Conversation
/// is made the way every managed Conversation is (ADR-0025).
///
/// # Errors
///
/// Returns a `not_found` `import.not_found`, a `conflict`
/// `import.already_resumed`, or what [`workspace::create`] and
/// [`workspace::create_in_local_checkout`] refuse.
pub(crate) async fn resume(
	tx: &mut WriteTransaction,
	actor: &Actor,
	resume: Resume,
	home: &WorkspaceHome,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Resume {
		import_id,
		retention,
		working_tree,
		prepared,
	} = resume;
	let Some(import) = tx.imported_conversation(import_id.0).await? else {
		return Err(CoreError::not_found(
			"import.not_found",
			"the Imported conversation does not exist",
		));
	};
	if import.resumed_as.is_some() {
		return Err(CoreError::conflict(
			"import.already_resumed",
			"a Conversation already continues that import; start its next Run \
			 there instead",
		));
	}
	let origin = ConversationOrigin::Imported { import_id };
	match (working_tree, prepared) {
		(
			WorkingTreeRequest::Workspace { .. },
			Prepared::Workspace(prepared),
		) => {
			workspace::create(
				tx,
				actor,
				retention,
				origin,
				prepared,
				home,
				now_unix_ms,
			)
			.await
		}
		(WorkingTreeRequest::LocalCheckout { project_id }, _) => {
			workspace::create_in_local_checkout(
				tx,
				actor,
				retention,
				origin,
				project_id,
				now_unix_ms,
			)
			.await
		}
		(
			WorkingTreeRequest::NoProject
			| WorkingTreeRequest::Workspace { .. },
			_,
		) => Err(CoreError::internal(
			"import.unprepared",
			"a Resume reached its transaction without what its working tree \
				 needs",
		)),
	}
}

impl From<ImportedConversationRecord> for ImportedConversation {
	fn from(record: ImportedConversationRecord) -> Self {
		Self {
			import_id: ImportId(record.import_id),
			harness: HarnessId(record.harness),
			native_conversation: NativeConversationId(
				record.native_conversation,
			),
			working_directory: record.working_directory.map(PathBuf::from),
			imported_by: Actor::from_record(record.imported_by).client_id(),
			imported_at: system_time(record.imported_at_unix_ms),
			resumed_as: record.resumed_as.map(ConversationId),
		}
	}
}

#[cfg(test)]
mod tests {
	use std::path::{Path, PathBuf};

	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::test_support::{
		FixedDiscovery, actor, conversation_snapshot, events,
		register_repository, request, start_core_discovering,
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
			panic!("expected QueryResult::ExternalConversations");
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
			panic!("expected CommandOutcome::ConversationImported");
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
			panic!("expected CommandOutcome::ConversationCreated");
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
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
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
		let core = start_core_discovering(
			&dir.path().join("plane.sqlite3"),
			discovery,
		)
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
			panic!("expected QueryResult::Conversations");
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
					working_directory: Some(PathBuf::from(
						"/home/jet/elsewhere"
					)),
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
					working_directory: Some(PathBuf::from(
						"/home/jet/elsewhere"
					)),
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
		let core = start_core_discovering(
			&dir.path().join("plane.sqlite3"),
			discovery,
		)
		.await;
		let project_id =
			register_repository(&core, &dir.path().join("repo")).await;
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
		let workspace =
			conversation_snapshot(&core, in_workspace.conversation_id)
				.await
				.workspace;
		let imports = external_conversations(&core).await.imported;

		assert_eq!(
			(
				refused(unplaced),
				refused(unknown),
				refused(unregistered),
				refused(twice),
				in_checkout.clone(),
				in_workspace.clone(),
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
					revision: in_checkout.revision,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::LocalCheckout { project_id },
					origin: ConversationOrigin::Imported {
						import_id: first.import_id,
					},
					name: in_checkout.name.clone(),
					created_at: in_checkout.created_at,
				},
				Conversation {
					conversation_id: in_workspace.conversation_id,
					revision: in_workspace.revision,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTree::Workspace { project_id },
					origin: ConversationOrigin::Imported {
						import_id: second.import_id,
					},
					name: in_workspace.name.clone(),
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
						name: Some(in_checkout.name.clone()),
					},
					EventKind::ConversationCreated {
						retention: RetentionPolicy::Retain,
						working_tree: WorkingTree::Workspace { project_id },
						origin: ConversationOrigin::Imported {
							import_id: second.import_id,
						},
						name: Some(in_workspace.name.clone()),
					},
				]
				.to_vec(),
			)
		);
	}
}
