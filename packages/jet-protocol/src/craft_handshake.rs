//! Restricted Craft startup before any Harness work may execute.
use crate::{
	CraftSpecification, NegotiatedProtocol, ProtocolOffer, ProtocolVersion,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Host-owned recovery context; never evidence that an unacknowledged action failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftResume {
	/// Version pinned when this execution first negotiated.
	pub version: ProtocolVersion,
	/// Native Conversation to reopen without replaying Commands automatically.
	pub native_conversation: String,
}

/// Immutable source context for a new native Harness Conversation. Receiving
/// this handshake is not authority to perform the fork before `Start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftFork {
	/// Native Harness Conversation that owns the selected source content.
	pub source_native_conversation: String,
	/// Jet Conversation that owns the selected Run.
	pub source_conversation_id: Uuid,
	/// Jet Run that owns the selected checkpoint.
	pub source_run_id: Uuid,
	/// One-based checkpoint boundary selected by the user.
	pub checkpoint_turn: u32,
	/// Checked-out source commit retained by the checkpoint.
	pub checkpoint_commit: String,
	/// Exact working-tree object retained by the checkpoint.
	pub checkpoint_tree: String,
}

/// First host control payload after the `jet-craft\n` preface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftHello {
	/// Host Craft-protocol support, independent of its GUI/helper protocols.
	pub protocol: ProtocolOffer,
	/// Host specification-schema support, negotiated independently.
	pub specification: ProtocolOffer,
	/// Stable identity persisted by the host before dispatching external work.
	pub execution_id: Uuid,
	/// Absent for a new execution; present only for explicit recovery.
	#[serde(default)]
	pub resume: Option<CraftResume>,
	/// Absent unless this new execution should fork an existing native
	/// Conversation from the selected immutable checkpoint.
	#[serde(default)]
	pub fork: Option<CraftFork>,
}

/// Successful negotiation; the host persists the version with the execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftReady {
	/// Selected Craft protocol and capabilities.
	pub protocol: NegotiatedProtocol,
	/// Independently selected specification schema minor.
	pub specification_protocol: NegotiatedProtocol,
	/// Declarations must match the host's accepted installed specification.
	pub specification: CraftSpecification,
	/// Understood features; unknown optional features remain disabled.
	pub enabled_features: Vec<String>,
}
