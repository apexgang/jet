//! Durable per-turn review limits and single-exchange admission (ADR-0012).
use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	ApprovalRequest, ApprovalReview, CoreError, ReviewDecision, ReviewOutcome,
	RunId, review::unavailable, run_state,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Guard {
	turn: u32,
	pending: Option<Uuid>,
	history: VecDeque<ReviewDecision>,
	stopped: bool,
	denied: Vec<Denied>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Denied {
	review_id: Uuid,
	request: ApprovalRequest,
	retry: Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Retry {
	NotGranted,
	Granted,
	Consumed,
}

impl Guard {
	pub(crate) fn admit(
		&mut self,
		turn: u32,
		review_id: Uuid,
		request: &ApprovalRequest,
	) -> Result<(), CoreError> {
		if self.turn != turn {
			*self = Self {
				turn,
				..Self::default()
			};
		}
		if self.pending.is_some() {
			return Err(unavailable("review.in_progress"));
		}
		// ASVS 8.3.1: execution parameters come from the stored denial.
		// A changed tool or action cannot consume the user's grant.
		let retry = self.denied.iter_mut().find(|denied| {
			denied.retry == Retry::Granted
				&& crate::review_action::same_action(&denied.request, request)
		});
		if let Some(denied) = retry {
			denied.retry = Retry::Consumed;
			self.pending = Some(review_id);
			return Ok(());
		}
		if self.stopped {
			return Err(unavailable("review.denial_budget"));
		}
		let operation = crate::review_action::operation(request);
		if self.denied.iter().any(|denied| {
			crate::review_action::operation(&denied.request) == operation
		}) {
			return Err(unavailable("review.workaround_denied"));
		}
		// ASVS 2.3.4: reserve before external work; restart leaves an
		// uncertain exchange held until the next turn, never replayed.
		self.pending = Some(review_id);
		Ok(())
	}

	pub(crate) fn authorize(
		&mut self,
		turn: u32,
		review_id: Uuid,
	) -> Result<(), CoreError> {
		let denied = self
			.denied
			.iter_mut()
			.find(|denied| {
				self.turn == turn
					&& denied.review_id == review_id
					&& denied.retry == Retry::NotGranted
			})
			.ok_or_else(|| unavailable("review.retry_unavailable"))?;
		denied.retry = Retry::Granted;
		Ok(())
	}

	pub(crate) fn record(&mut self, review: &ApprovalReview) {
		if self.pending == Some(review.review_id) {
			self.pending = None;
		}
		let decision = match review.outcome {
			ReviewOutcome::Decided { decision, .. } => decision,
			ReviewOutcome::Denied { .. } => ReviewDecision::Deny,
			// A reviewer failure cannot erase consecutive denials.
			ReviewOutcome::Unavailable { .. } => return,
		};
		if decision == ReviewDecision::Deny
			&& !self
				.denied
				.iter()
				.any(|denied| denied.review_id == review.review_id)
		{
			// Every denied review remains independently grantable, even
			// after the budget stops. Re-recording cannot replenish a grant.
			self.denied.push(Denied {
				review_id: review.review_id,
				request: review.request.clone(),
				retry: Retry::NotGranted,
			});
		}
		if self.stopped {
			return;
		}
		self.history.push_back(decision);
		if self.history.len() > 50 {
			self.history.pop_front();
		}
		// ASVS 2.3.2: the stop latches for the rest of this turn.
		self.stopped = self
			.history
			.iter()
			.rev()
			.take_while(|d| **d == ReviewDecision::Deny)
			.count() >= 3
			|| self
				.history
				.iter()
				.filter(|d| **d == ReviewDecision::Deny)
				.count() >= 10;
	}
}

pub(crate) fn turn(state: &run_state::State) -> Result<u32, CoreError> {
	state
		.changes
		.as_ref()
		.filter(|tracking| tracking.active.is_some() && !state.disconnected)
		.map(|tracking| tracking.completed)
		.ok_or_else(|| unavailable("review.turn_unavailable"))
}

pub(crate) async fn load(
	tx: &mut jet_store::ReadTransaction,
	run_id: RunId,
) -> Result<run_state::State, CoreError> {
	let run = tx
		.run(run_id.0)
		.await?
		.ok_or_else(|| unavailable("review.run_unavailable"))?;
	if run.lifecycle != crate::RunLifecycle::Active {
		return Err(unavailable("review.run_unavailable"));
	}
	let record = tx
		.run_execution(run_id.0)
		.await?
		.ok_or_else(|| unavailable("review.run_unavailable"))?;
	run_state::decode(&record.state)
}
