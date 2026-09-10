//! Command attribution and deduplication receipts.

use super::{column_error, parse_uuid};
use crate::StoreError;
use uuid::Uuid;

/// The authenticated origin of a Command or Event (ADR-0063).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorRecord {
	/// An interactive GUI client identified by its durable Client identity.
	InteractiveClient {
		/// The client's durable identity.
		client_id: Uuid,
	},
}

impl ActorRecord {
	pub(crate) fn columns(self) -> (&'static str, Uuid) {
		match self {
			Self::InteractiveClient { client_id } => {
				("interactive_client", client_id)
			}
		}
	}

	pub(crate) fn parse(kind: &str, id: &str) -> Result<Self, StoreError> {
		match kind {
			"interactive_client" => Ok(Self::InteractiveClient {
				client_id: parse_uuid("actor_id", id)?,
			}),
			_ => Err(column_error(
				"actor_kind",
				format!("unknown actor {kind:?} with id {id:?}"),
			)),
		}
	}
}

/// A durable receipt for one accepted Actor-scoped Command identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReceiptRecord {
	/// The authenticated Actor that submitted the Command.
	pub actor: ActorRecord,
	/// The Actor-scoped Command identity.
	pub command_id: Uuid,
	/// SHA-256 digest of the request content, discarded after thirty days.
	pub request_digest: Option<[u8; 32]>,
	/// When the Command was accepted.
	pub recorded_at_unix_ms: i64,
	/// Version of the private outcome encoding, discarded after thirty days.
	pub outcome_version: Option<u32>,
	/// Encoded authoritative outcome, discarded after thirty days.
	pub outcome: Option<String>,
}

/// A Command receipt to record in the accepting state transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommandReceipt {
	/// The authenticated Actor that submitted the Command.
	pub actor: ActorRecord,
	/// The Actor-scoped Command identity.
	pub command_id: Uuid,
	/// SHA-256 digest of the request content.
	pub request_digest: [u8; 32],
	/// When the Command was accepted.
	pub recorded_at_unix_ms: i64,
	/// Version of the private outcome encoding.
	pub outcome_version: u32,
	/// Encoded authoritative outcome.
	pub outcome: String,
}
