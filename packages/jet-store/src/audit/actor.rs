//! Audit attribution is separate from authenticated Command authority.
use crate::{ActorRecord, StoreError};
use uuid::Uuid;

/// The origin of a Security-audit decision, without granting Command authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditActorRecord {
	/// An authenticated interactive Client identity.
	InteractiveClient {
		/// The client's durable identity.
		client_id: Uuid,
	},
	/// Jet applied verified release metadata on this Plane.
	CraftRevocation,
	/// The retention sweep acted on a Conversation's own policy or a
	/// grace period that ended (ADR-0015).
	Retention,
}

impl AuditActorRecord {
	pub(crate) fn columns(self) -> (&'static str, Uuid) {
		match self {
			Self::InteractiveClient { client_id } => {
				("interactive_client", client_id)
			}
			Self::CraftRevocation => ("craft_revocation", Uuid::nil()),
			Self::Retention => ("retention", Uuid::nil()),
		}
	}

	pub(crate) fn parse(kind: &str, id: &str) -> Result<Self, StoreError> {
		if id == Uuid::nil().to_string() {
			match kind {
				"craft_revocation" => return Ok(Self::CraftRevocation),
				"retention" => return Ok(Self::Retention),
				_ => {}
			}
		}
		ActorRecord::parse(kind, id).map(Self::from)
	}
}

impl From<ActorRecord> for AuditActorRecord {
	fn from(actor: ActorRecord) -> Self {
		match actor {
			ActorRecord::InteractiveClient { client_id } => {
				Self::InteractiveClient { client_id }
			}
		}
	}
}
