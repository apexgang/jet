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

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jet_store::{
	ImportedConversationRecord, NewImportedConversation, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capability::HarnessId;
use crate::command::CommandOutcome;
use crate::conversation::{ConversationId, ConversationOrigin};
use crate::error::CoreError;
use crate::event::{EventKind, EventSequence, EventSubject};
use crate::filesystem::canonicalize;
use crate::preparation::Prepared;
use crate::project::Project;
use crate::query::QueryResult;
use crate::workspace::{self, WorkingTreeRequest, WorkspaceHome};
use crate::{Actor, ClientId, Core, ProjectId, system_time};
use jet_store::RetentionPolicy;

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

/// Lists what the Plane can see and what it holds.
///
/// Discovery runs outside the store's lock; the imports, the Projects the
/// discovered directories are placed against, and the Event fence come
/// from one SQLite snapshot (ASVS 2.3.3).
pub(crate) async fn external_conversations(
	core: &Core,
) -> Result<QueryResult, CoreError> {
	let mut discovered = core.discovery.discover().await;
	for found in &mut discovered {
		found.working_directory = resolve(found.working_directory.take()).await;
	}
	core.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let projects: Vec<Project> =
				tx.projects().await?.into_iter().map(Into::into).collect();
			let imported: Vec<ImportedConversation> = tx
				.imported_conversations()
				.await?
				.into_iter()
				.map(Into::into)
				.collect();
			let discovered = discovered
				.into_iter()
				.map(|found| present(found, &projects, &imported))
				.collect();
			Ok(QueryResult::ExternalConversations(
				ExternalConversationList {
					cursor,
					discovered,
					imported,
				},
			))
		})
		.await
}

/// The directory as the filesystem names it, so it compares with Project
/// roots, which are canonical. A directory that is gone, or that was never
/// reported, is kept as reported: what the Harness said is still metadata.
async fn resolve(directory: Option<PathBuf>) -> Option<PathBuf> {
	let directory = directory?;
	Some(canonicalize(directory.clone()).await.unwrap_or(directory))
}

/// Places one discovered identity against the Plane's Projects and imports.
fn present(
	found: DiscoveredConversation,
	projects: &[Project],
	imported: &[ImportedConversation],
) -> ExternalConversation {
	let DiscoveredConversation {
		harness,
		native_conversation,
		working_directory,
		process,
	} = found;
	let import_id = imported
		.iter()
		.find(|import| {
			import.harness == harness
				&& import.native_conversation == native_conversation
		})
		.map(|import| import.import_id);
	let origin = match working_directory {
		Some(working_directory) => match covering(projects, &working_directory)
		{
			Some(project_id) => ExternalOrigin::Project {
				project_id,
				working_directory,
			},
			None => ExternalOrigin::Unregistered { working_directory },
		},
		None => ExternalOrigin::Unknown,
	};
	ExternalConversation {
		harness,
		native_conversation,
		origin,
		process,
		import_id,
	}
}

/// The Project whose root holds `directory`, preferring the deepest root
/// when a Project sits inside another (ADR-0103).
fn covering(projects: &[Project], directory: &Path) -> Option<ProjectId> {
	projects
		.iter()
		.filter(|project| directory.starts_with(&project.root))
		.max_by_key(|project| project.root.components().count())
		.map(|project| project.project_id)
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
#[path = "import_tests.rs"]
mod tests;
