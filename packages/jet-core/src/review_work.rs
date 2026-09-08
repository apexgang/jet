//! Reviewing one held approval request, and recording what was decided
//! (ADR-0012, ADR-0105).
//!
//! The Harness is waiting while this runs, so a review is carried out
//! beside the execution it is about rather than through the Effect queue:
//! the answer is only worth anything to the connection that is still
//! holding the request. What outlives it is the record — the exact request,
//! the exact reviewer, and the decision the core made — committed before
//! that decision is delivered.
//!
//! Nothing here widens a boundary. The reviewer never reaches the Harness,
//! and the only thing that crosses back is one decision for one request.

use std::{sync::Arc, time::Duration};

use uuid::Uuid;

use crate::{
	ApprovalRequest, ApprovalReview, Core, CoreError, EventKind,
	ReviewDecision, ReviewHost, ReviewInput, ReviewOutcome, ReviewerSelection,
	RunId, review::unavailable, review_policy::Selection,
};

/// How long one reviewer request may take before the request is left for a
/// person. It is the whole exchange, including model selection.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(30);
/// Most bytes a reviewer answer may occupy.
const OUTPUT_BYTES: usize = 4096;
/// Longest reviewer Model identity this core will pin.
const MODEL_BYTES: usize = 128;
/// Longest opaque Adapter pin one selection may carry.
const ADAPTER_STATE_BYTES: usize = 16 * 1024;

impl Core {
	/// Installs the trusted reviewer Adapter before Automatic review runs.
	#[must_use]
	pub fn with_review_host(mut self, host: Arc<dyn ReviewHost>) -> Self {
		self.review_host = Some(host);
		self
	}

	/// Reviews one held request beside its live execution.
	///
	/// The supervising loop keeps reading the Craft while this runs, so a
	/// slow reviewer never stalls the source the Run is still producing.
	pub(crate) fn begin_automatic_review(
		self: &Arc<Self>,
		run_id: RunId,
		request: ApprovalRequest,
	) {
		let core = Arc::clone(self);
		tokio::spawn(async move {
			let _ = core.review_approval(run_id, request).await;
		});
	}

	/// Carries out one review, records it, and delivers the decision it
	/// made. A review that could not happen is recorded as one and leaves
	/// the request waiting for a person.
	async fn review_approval(
		&self,
		run_id: RunId,
		request: ApprovalRequest,
	) -> Result<(), CoreError> {
		request.validate()?;
		let Some(routing) = self.review_routing(run_id).await? else {
			return Ok(());
		};
		let answered = match routing.selection {
			Ok(selection) => {
				match tokio::time::timeout(
					REVIEW_TIMEOUT,
					self.consult(run_id, &request, &selection),
				)
				.await
				{
					Ok(Ok(reviewed)) => reviewed.settled(),
					Ok(Err(error)) => Answered::unavailable(error),
					Err(_) => {
						Answered::unavailable(unavailable("review.timeout"))
					}
				}
			}
			Err(error) => Answered::unavailable(error),
		};
		let review = ApprovalReview {
			review_id: Uuid::now_v7(),
			run_id,
			request,
			reviewer: answered.reviewer,
			provider: routing.provider,
			binding_id: routing.binding_id,
			model: answered.model,
			policy: routing.policy,
			outcome: answered.outcome,
		};
		// ADR-0064: the decision is durable before it can authorize
		// anything, so a lost delivery leaves the request held rather than
		// leaving an allowance nobody recorded.
		self.record_review(&review).await?;
		let ReviewOutcome::Decided { decision, .. } = review.outcome else {
			return Ok(());
		};
		let connection = self
			.live_connection(run_id)
			.ok_or_else(|| unavailable("review.execution_unavailable"))?;
		connection
			.decide_approval(&review.request.request_id, decision)
			.await
	}

	/// One reviewer exchange: pin the reviewer, read the bounded input,
	/// and validate whatever comes back.
	async fn consult(
		&self,
		run_id: RunId,
		request: &ApprovalRequest,
		selection: &Selection,
	) -> Result<Reviewed, CoreError> {
		let host = self
			.review_host
			.as_ref()
			.ok_or_else(|| unavailable("review.host_unavailable"))?;
		let pinned = host
			.select(&selection.craft, &selection.binding)
			.await
			.map_err(|_| unavailable("review.model_unavailable"))?;
		validate_selection(&pinned)?;
		let input = self.review_input(run_id, request).await?;
		validate_input(&input)?;
		let reply = host
			.review(&selection.craft, &selection.binding, &pinned, &input)
			.await
			.map_err(|_| unavailable("review.failed"))?;
		if reply.model != pinned.model || reply.output.len() > OUTPUT_BYTES {
			return Err(unavailable("review.output_invalid"));
		}
		Ok(Reviewed {
			reviewer: reply.reviewer,
			model: pinned.model,
			verdict: crate::review_output::validate(&reply.output)?,
		})
	}

	/// Commits one review to the Event journal and the Security audit. The
	/// audit keeps attribution and outcome; the exact action stays in the
	/// journal, because the audit holds no Conversation content (ADR-0105).
	async fn record_review(
		&self,
		review: &ApprovalReview,
	) -> Result<(), CoreError> {
		let now = self.now_unix_ms();
		let run_id = review.run_id;
		let outcome = match &review.outcome {
			ReviewOutcome::Decided {
				decision: ReviewDecision::Allow,
				..
			} => jet_store::AuditOutcome::Succeeded,
			ReviewOutcome::Decided {
				decision: ReviewDecision::Deny,
				..
			} => jet_store::AuditOutcome::Denied,
			// Nothing was decided, so nothing was allowed or refused.
			ReviewOutcome::Unavailable { .. } => {
				jet_store::AuditOutcome::Failed
			}
		};
		let review = review.clone();
		self.store
			.write(async |tx| {
				let run = tx
					.run(run_id.0)
					.await?
					.ok_or_else(|| unavailable("review.run_unavailable"))?;
				let record = tx
					.run_execution(run_id.0)
					.await?
					.ok_or_else(|| unavailable("review.run_unavailable"))?;
				let plan: crate::LaunchPlan =
					crate::run_state::decode(&record.plan)?;
				crate::run_state::append(
					tx,
					&crate::EventActor::RunSupervisor {
						run_id,
						authorized_by: plan.client_id,
					},
					&run.into(),
					EventKind::ApprovalReviewed {
						review: Box::new(review),
					},
					now,
				)
				.await?;
				crate::audit::record(
					tx,
					&crate::Actor::InteractiveClient {
						client_id: plan.client_id,
					},
					crate::audit::Decision {
						decision: crate::AuditDecision::ApprovalReviewed,
						subject: crate::audit::AuditSubject::Execution(run_id),
						outcome,
					},
					now,
				)
				.await
			})
			.await
	}
}

/// One completed reviewer exchange, before the core settles it.
struct Reviewed {
	reviewer: crate::Reviewer,
	model: String,
	verdict: crate::ReviewVerdict,
}

/// What the review came to, and who it was that answered. A review that
/// never reached a reviewer names none, because naming one would claim a
/// judgement nothing made.
struct Answered {
	reviewer: Option<crate::Reviewer>,
	model: Option<String>,
	outcome: ReviewOutcome,
}

impl Reviewed {
	/// Applies the core's own rules to the reviewer's judgement.
	fn settled(self) -> Answered {
		let decision = crate::review_output::settle(&self.verdict);
		Answered {
			reviewer: Some(self.reviewer),
			model: Some(self.model),
			outcome: ReviewOutcome::Decided {
				verdict: self.verdict,
				decision,
			},
		}
	}
}

impl Answered {
	/// A review that did not happen. Nothing was allowed, and the held
	/// request still waits for a person.
	fn unavailable(error: CoreError) -> Self {
		Self {
			reviewer: None,
			model: None,
			outcome: ReviewOutcome::Unavailable { reason: error.code },
		}
	}
}

fn validate_selection(selection: &ReviewerSelection) -> Result<(), CoreError> {
	if selection.model.is_empty()
		|| selection.model.len() > MODEL_BYTES
		|| selection.model.chars().any(char::is_control)
		|| selection.adapter_state.len() > ADAPTER_STATE_BYTES
	{
		return Err(unavailable("review.model_invalid"));
	}
	Ok(())
}

fn validate_input(input: &ReviewInput) -> Result<(), CoreError> {
	if input.transcript.len() > crate::review::TRANSCRIPT_BYTES
		|| input.tool.len() > crate::review::TOOL_BYTES
		|| input.action.len() > crate::review::ACTION_BYTES
	{
		return Err(unavailable("review.input_invalid"));
	}
	Ok(())
}
