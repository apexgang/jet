//! What a Command does before its transaction opens.
//!
//! A Command that must look at the machine does so here, beside the
//! Capability revalidation of ADR-0086: outside the store's lock, so a slow
//! filesystem or a slow tool stalls one Command rather than every Query on
//! the Plane, and before any receipt exists, so a refusal that describes
//! the world rather than the Command is not replayed for thirty days
//! (ADR-0093).

use crate::audit;
use crate::command::{Command, CommandId};
use crate::error::CoreError;
use crate::import::{self, DiscoveredConversation};
use crate::project::{self, Registrable};
use crate::promotion_command::{self, PreparedPromotion};
use crate::workspace::{self, PreparedWorkspace, WorkingTreeRequest};
use crate::{Actor, Core};

/// What the preparation of one Command produced for its transaction.
pub(crate) enum Prepared {
	/// A Run pinned to an accepted Craft and validated working tree.
	Run(crate::run_command::LaunchPlan),
	/// The Command needs nothing from outside the store.
	Nothing,
	/// The root a Path grant resolved to and `git` accepted.
	Registration(Registrable),
	/// The Project, resolved base, and captured seed a new Workspace
	/// starts from.
	Workspace(PreparedWorkspace),
	/// A promotion whose binding still matches the repository.
	Promotion(PreparedPromotion),
	/// The identity an import names, as discovery reports it right now.
	Import(DiscoveredConversation),
}

impl Core {
	/// Admits `command` to its transaction: revalidates the Capabilities it
	/// depends on (ADR-0086) and prepares what it needs from outside the
	/// store.
	///
	/// A Command whose outcome is already durable is neither revalidated
	/// nor prepared: its work is done, and repeating it must return what
	/// the Plane decided then rather than what the machine would decide now
	/// (ADR-0093).
	///
	/// # Errors
	///
	/// Returns the refusal, which is answered without a receipt and, when
	/// the Command is one the Security audit records, recorded there as
	/// denied (ADR-0105).
	pub(crate) async fn admit(
		&self,
		actor: &Actor,
		command_id: CommandId,
		command: &Command,
		now_unix_ms: i64,
	) -> Result<Prepared, CoreError> {
		let actor_record = actor.record();
		let recorded = self
			.store
			.read(async |tx| {
				Ok::<_, CoreError>(
					tx.command_receipt(actor_record, command_id.0)
						.await?
						.is_some(),
				)
			})
			.await?;
		if recorded {
			return Ok(Prepared::Nothing);
		}
		let admitted = match self.revalidate_capabilities(command).await {
			Ok(()) => self.prepare(actor, command).await,
			Err(refusal) => Err(refusal),
		};
		match admitted {
			Ok(prepared) => Ok(prepared),
			Err(refusal) => {
				// ASVS 16.2.1: a decision the audit would have recorded is
				// recorded when it is refused as well (ADR-0105).
				audit::record_refusal(&self.store, actor, command, now_unix_ms)
					.await?;
				Err(refusal)
			}
		}
	}

	/// Prepares `command` for its transaction.
	///
	/// # Errors
	///
	/// Returns the refusal the preparation produced.
	async fn prepare(
		&self,
		actor: &Actor,
		command: &Command,
	) -> Result<Prepared, CoreError> {
		match command {
			Command::StartRun {
				conversation_id,
				craft,
				prompt,
			} => Ok(Prepared::Run(
				crate::run_command::prepare(
					self,
					actor,
					*conversation_id,
					craft,
					prompt,
				)
				.await?,
			)),
			Command::RegisterProject { grant } => Ok(Prepared::Registration(
				project::prepare_registration(actor, grant).await?,
			)),
			Command::CreateConversation {
				working_tree:
					WorkingTreeRequest::Workspace {
						project_id,
						base,
						seed,
					},
				..
			} => Ok(Prepared::Workspace(
				workspace::prepare(self, *project_id, base, seed).await?,
			)),
			Command::PromoteWorkspace { binding } => Ok(Prepared::Promotion(
				promotion_command::prepare(self, actor, binding).await?,
			)),
			Command::ImportConversation {
				harness,
				native_conversation,
			} => Ok(Prepared::Import(
				import::prepare_import(self, harness, native_conversation)
					.await?,
			)),
			Command::ResumeImportedConversation { working_tree, .. } => {
				import::require_working_tree(working_tree)?;
				match working_tree {
					WorkingTreeRequest::Workspace {
						project_id,
						base,
						seed,
					} => Ok(Prepared::Workspace(
						workspace::prepare(self, *project_id, base, seed)
							.await?,
					)),
					WorkingTreeRequest::NoProject
					| WorkingTreeRequest::LocalCheckout { .. } => Ok(Prepared::Nothing),
				}
			}
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
