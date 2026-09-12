//! Audit origins do not authorize Commands or Queries.
use crate::ClientId;

/// The responsible origin of a Security-audit decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditActor {
	/// An authenticated interactive Client identity.
	InteractiveClient {
		/// The client's durable identity.
		client_id: ClientId,
	},
	/// Jet applied signed release revocations on this Plane.
	CraftRevocation,
	/// The retention sweep forgot or deleted a Conversation (ADR-0015).
	Retention,
}

impl From<jet_store::AuditActorRecord> for AuditActor {
	fn from(actor: jet_store::AuditActorRecord) -> Self {
		match actor {
			jet_store::AuditActorRecord::InteractiveClient { client_id } => {
				Self::InteractiveClient {
					client_id: ClientId(client_id),
				}
			}
			jet_store::AuditActorRecord::CraftRevocation => {
				Self::CraftRevocation
			}
			jet_store::AuditActorRecord::Retention => Self::Retention,
		}
	}
}
