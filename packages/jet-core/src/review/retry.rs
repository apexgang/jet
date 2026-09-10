//! Authenticated one-shot retry grants. Grants never authorize execution.
use crate::{
	Actor, AuditDecision, CommandOutcome, CoreError, EventActor, EventKind,
	RunId, audit, review::guard as review_guard, run::state as run_state,
};
use jet_store::WriteTransaction;
use uuid::Uuid;

pub(crate) async fn authorize(
	tx: &mut WriteTransaction,
	actor: &Actor,
	run_id: RunId,
	review_id: Uuid,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let mut state = review_guard::load(tx, run_id).await?;
	let result = state
		.review
		.authorize(review_guard::turn(&state)?, review_id);
	// ASVS 16.3.2: both grants and refused repeated grants are audited,
	// without copying the action or any Conversation content.
	audit::record(
		tx,
		actor,
		audit::Decision {
			decision: AuditDecision::ApprovalRetryAuthorized,
			subject: audit::AuditSubject::Execution(run_id),
			outcome: if result.is_ok() {
				crate::AuditOutcome::Succeeded
			} else {
				crate::AuditOutcome::Denied
			},
		},
		now,
	)
	.await?;
	result?;
	run_state::save(tx, run_id, &state).await?;
	let run = tx
		.run(run_id.0)
		.await?
		.ok_or_else(|| crate::review::unavailable("review.run_unavailable"))?;
	run_state::append(
		tx,
		&EventActor::InteractiveClient {
			client_id: actor.client_id(),
		},
		&run.into(),
		EventKind::ApprovalRetryAuthorized { review_id },
		now,
	)
	.await?;
	Ok(CommandOutcome::ApprovalRetryAuthorized { review_id })
}
