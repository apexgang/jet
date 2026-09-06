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
