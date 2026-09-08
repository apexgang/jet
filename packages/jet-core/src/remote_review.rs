//! Exact-action destination review; approval never grants another operation.
use crate::{
	Actor, ClientId, CommandOutcome, CoreError, RemoteToolDecision,
	RemoteToolRequest,
};
use jet_store::{ReadTransaction, WriteTransaction};
use uuid::Uuid;

pub(crate) async fn pending(
	tx: &mut ReadTransaction,
	client_id: ClientId,
	operation_id: Uuid,
	now: i64,
) -> Result<RemoteToolRequest, CoreError> {
	let record = tx
		.remote_operation(&client_id.0.to_string(), &operation_id.to_string())
		.await?
		.ok_or_else(missing)?;
	if record.state != "pending"
		|| now.saturating_sub(record.recorded_at_unix_ms) > 600_000
	{
		return Err(missing());
	}
	serde_json::from_str(record.request.as_deref().ok_or_else(missing)?)
		.map_err(crate::remote_tool::codec)
}

pub(crate) async fn review(
	tx: &mut WriteTransaction,
	actor: &Actor,
	client_id: ClientId,
	operation_id: Uuid,
	decision: RemoteToolDecision,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let request = pending(tx, client_id, operation_id, now).await?;
	let (state, outcome) = match decision {
		RemoteToolDecision::AllowOnce => {
			("approved", crate::AuditOutcome::Succeeded)
		}
		RemoteToolDecision::Deny => ("denied", crate::AuditOutcome::Denied),
	};
	// ASVS 8.3.1: this GUI Command chooses only a decision; the exact action
	// comes from the immutable request captured before review was requested.
	tx.transition_remote_operation(
		&client_id.0.to_string(),
		&operation_id.to_string(),
		"pending",
		state,
	)
	.await?;
	crate::remote_tool::audit(
		tx,
		actor,
		client_id,
		&request,
		"remote.reviewed",
		outcome,
		now,
	)
	.await?;
	Ok(CommandOutcome::RemoteToolReviewed { operation_id })
}

fn missing() -> CoreError {
	CoreError::not_found(
		"remote.review_unavailable",
		"no unexpired remote action awaits this review",
	)
}
