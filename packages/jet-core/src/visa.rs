//! Visa admission belongs to the destination's authoritative Core (ADR-0062).
use crate::{
	AccountBindingId, Actor, ConversationId, Core, CoreError, LaunchPlan,
	PlaneId,
};
use serde::{Deserialize, Serialize};

/// Explicit Plane-local selections for a new native execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaRunRequest {
	/// Conversation already owned by the destination.
	pub conversation_id: ConversationId,
	/// Selected destination, which must be this Conversation's Home Plane.
	pub destination_plane_id: PlaneId,
	/// Selected binding in the destination's own store.
	pub account_binding_id: AccountBindingId,
	/// Accepted installed Craft identity.
	pub craft: String,
	/// Authorized initial input.
	pub prompt: String,
}

/// Immutable destination and authentication selection retained with a Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaSelection {
	/// Home Plane on which the native processes execute.
	pub plane_id: PlaneId,
	/// Destination-local binding selected at admission.
	pub account_binding_id: AccountBindingId,
}

pub(crate) async fn prepare(
	core: &Core,
	actor: &Actor,
	request: &VisaRunRequest,
) -> Result<LaunchPlan, CoreError> {
	let selection = VisaSelection {
		plane_id: request.destination_plane_id,
		account_binding_id: request.account_binding_id,
	};
	let binding = core
		.store
		.read(async |tx| selection.binding(tx).await)
		.await?;
	let mut plan = crate::run_command::prepare(
		core,
		actor,
		request.conversation_id,
		&request.craft,
		&request.prompt,
	)
	.await?;
	validate_account(core, &plan.craft, &binding).await?;
	plan.visa = Some(selection);
	// Older daemons must not ignore this binding when admitting queued work.
	plan.version = 2;
	Ok(plan)
}

impl VisaSelection {
	pub(crate) async fn binding(
		&self,
		tx: &mut jet_store::ReadTransaction,
	) -> Result<crate::AccountBinding, CoreError> {
		// ASVS 2.2.3, 8.3.1: a destination is a precondition on local
		// authority, never permission to forward work to another Plane.
		if tx.plane().await?.plane_id != self.plane_id.0 {
			return Err(CoreError::conflict(
				"visa.destination_mismatch",
				"Visa execution requires the Conversation's Home Plane; transfer it explicitly to change Planes",
			));
		}
		tx.account_binding(self.account_binding_id.0)
			.await?
			.map(Into::into)
			.ok_or_else(|| {
				CoreError::conflict(
					"visa.binding_unavailable",
					"the selected Account binding is unavailable on this Plane",
				)
			})
	}
}

impl Core {
	pub(crate) async fn revalidate_visa(
		&self,
		plan: &LaunchPlan,
	) -> Result<(), CoreError> {
		let Some(selection) = plan.visa else {
			return Ok(());
		};
		let binding = self
			.store
			.read(async |tx| selection.binding(tx).await)
			.await?;
		// ASVS 8.3.2: re-read the pinned binding for each new native launch,
		// including queued work. Recovery of an existing process grants no launch.
		validate_account(self, &plan.craft, &binding).await
	}
}

async fn validate_account(
	core: &Core,
	craft: &crate::PinnedCraft,
	binding: &crate::AccountBinding,
) -> Result<(), CoreError> {
	// ASVS 16.5.3: a selected credential source must never fall through to
	// unrelated native authentication inherited by the destination processes.
	if binding.credential_reference != crate::CredentialReference::HarnessNative
	{
		return Err(CoreError::conflict(
			"visa.credential_source_unsupported",
			"Visa execution currently requires native Harness authentication on the destination",
		));
	}
	let host = core.run_host.as_ref().ok_or_else(|| {
		CoreError::conflict(
			"craft.unavailable",
			"no Run transport was configured",
		)
	})?;
	if host.native_provider(craft)? != binding.provider {
		return Err(CoreError::conflict(
			"visa.provider_mismatch",
			"the Account binding's Provider does not match the selected Harness",
		));
	}
	Ok(())
}
