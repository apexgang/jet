//! Wire form of external Conversations and imports (ADR-0010).
//!
//! An external Conversation is a Harness-native identity the Plane can see
//! outside its management. An import registers one as metadata; a managed
//! Resume continues it as a new Conversation in a Workspace or Local
//! checkout of a registered Project. The Plane never seizes the process
//! that holds an external Conversation: live takeover is reported only
//! where the Harness advertises a cooperating structured endpoint.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::Actor;

/// A process outside the Plane's management that holds an external
/// Conversation live, and what the Plane can do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExternalProcess {
	/// No live process was observed. The identity can be continued only by
	/// a new managed Run.
	None,
	/// A live process the Plane can see only through a terminal. It stays
	/// external.
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
		endpoint: String,
	},
}

/// Where an external Conversation did its work, as it relates to the
/// Plane's Projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExternalOrigin {
	/// Inside a registered Project, which a Resume may select directly.
	Project {
		/// The Project whose root holds the directory.
		project_id: Uuid,
		/// The directory the Harness reported.
		working_directory: String,
	},
	/// In a directory no Project covers. The user registers it, or maps
	/// another Project, before a Resume.
	Unregistered {
		/// The directory the Harness reported.
		working_directory: String,
	},
	/// The Harness did not say where it worked.
	Unknown,
}

/// One Harness-native Conversation the Plane can see outside its
/// management.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalConversation {
	/// The Harness whose identity it is, such as `codex`.
	pub harness: String,
	/// The identity as the Harness spells it.
	pub native_conversation: String,
	/// Where it did its work.
	pub origin: ExternalOrigin,
	/// The live process holding it, if any.
	pub process: ExternalProcess,
	/// The import that already registered it, if one has.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub import_id: Option<Uuid>,
}

/// One Imported conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedConversation {
	/// Durable identity.
	pub import_id: Uuid,
	/// The Harness whose identity it is.
	pub harness: String,
	/// The identity as the Harness spells it.
	pub native_conversation: String,
	/// The directory the Harness reported working in when it was imported.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub working_directory: Option<String>,
	/// The interactive user who imported it.
	pub imported_by: Actor,
	/// When it was imported, in signed Unix milliseconds.
	pub imported_at_unix_ms: i64,
	/// The Conversation that continues it, once a Resume has made one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resumed_as: Option<Uuid>,
}

/// The external Conversations the Plane can see and the imports it holds,
/// fenced by the journal position the imports were read at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalConversationList {
	/// Newest Event sequence visible when the imports were read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// Every identity the Plane can see right now.
	pub discovered: Vec<ExternalConversation>,
	/// Every import the Plane holds, in the order they were made.
	pub imported: Vec<ImportedConversation>,
}

/// Where a Conversation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationOrigin {
	/// Created in Jet.
	New,
	/// Created by a managed Resume to continue an Imported conversation.
	Imported {
		/// The import it continues.
		import_id: Uuid,
	},
}
