//! What a Command does before its transaction opens.
//!
//! A Command that must look at the machine does so here, beside the
//! Capability revalidation of ADR-0086: outside the store's lock, so a slow
//! filesystem or a slow tool stalls one Command rather than every Query on
//! the Plane, and before any receipt exists, so a refusal that describes
//! the world rather than the Command is not replayed for thirty days
//! (ADR-0093).

use crate::command::Command;
use crate::error::CoreError;
use crate::project::{self, Registrable};
use crate::{Actor, Core};

/// What the preparation of one Command produced for its transaction.
pub(crate) enum Prepared {
	/// The Command needs nothing from outside the store.
	Nothing,
	/// The root a Path grant resolved to and `git` accepted.
	Registration(Registrable),
}

impl Core {
	/// Prepares `command` for its transaction.
	///
	/// # Errors
	///
	/// Returns the refusal the preparation produced, which is answered
	/// without a receipt.
	pub(crate) async fn prepare(
		&self,
		actor: &Actor,
		command: &Command,
	) -> Result<Prepared, CoreError> {
		match command {
			Command::RegisterProject { grant } => Ok(Prepared::Registration(
				project::prepare_registration(actor, grant).await?,
			)),
			Command::CreateConversation { .. }
			| Command::CreateRun { .. }
			| Command::SetSetting { .. }
			| Command::ClearSetting { .. }
			| Command::BindAccount { .. }
			| Command::UnbindAccount { .. }
			| Command::BeginAuditEpoch
			| Command::SetPairingGate { .. }
			| Command::OpenPairing { .. }
			| Command::ClaimPairing { .. }
			| Command::ConfirmPairing { .. }
			| Command::CompletePairing { .. }
			| Command::SetPairedClientAccess { .. }
			| Command::RevokePairedClient { .. }
			| Command::TransitionRun { .. } => Ok(Prepared::Nothing),
		}
	}
}
