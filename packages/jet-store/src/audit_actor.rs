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
}

impl AuditActorRecord {
	pub(crate) fn columns(self) -> (&'static str, Uuid) {
		match self {
			Self::InteractiveClient { client_id } => {
				("interactive_client", client_id)
			}
			Self::CraftRevocation => ("craft_revocation", Uuid::nil()),
		}
	}

	pub(crate) fn parse(kind: &str, id: &str) -> Result<Self, StoreError> {
		if kind == "craft_revocation" && id == Uuid::nil().to_string() {
			return Ok(Self::CraftRevocation);
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
