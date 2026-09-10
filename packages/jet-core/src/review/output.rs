//! Strict core validation of an untrusted reviewer answer, and what the
//! trusted core lets that answer authorize (ADR-0012).
//!
//! The reviewer returns a judgement. The decision is made here: an allow
//! stands only where policy already says it may, so a reviewer that says
//! yes to something it should not have cannot widen anything by saying it.
//! There is no decoder for anything but this one shape, and nothing in the
//! answer names a Command, a request, or a Run.

use crate::{
	CoreError, ReviewAuthorization, ReviewDecision, ReviewRisk, ReviewVerdict,
	review::{RATIONALE_BYTES, unavailable},
};
use serde::Deserialize;

/// Reads one reviewer answer, refusing anything but the exact shape.
///
/// # Errors
/// Returns a conflict for malformed JSON, unknown fields, unknown
/// vocabulary, or a rationale outside its bounds. A refusal is never an
/// allowance: the held request stays held.
pub(crate) fn validate(bytes: &[u8]) -> Result<ReviewVerdict, CoreError> {
	#[derive(Deserialize)]
	#[serde(deny_unknown_fields)]
	struct Answer {
		risk: ReviewRisk,
		authorization: ReviewAuthorization,
		decision: ReviewDecision,
		rationale: String,
	}
	let invalid = || unavailable("review.output_invalid");
	let answer: Answer =
		serde_json::from_slice(bytes).map_err(|_| invalid())?;
	if answer.rationale.trim().is_empty()
		|| answer.rationale.len() > RATIONALE_BYTES
		|| answer.rationale.chars().any(char::is_control)
	{
		return Err(invalid());
	}
	Ok(ReviewVerdict {
		risk: answer.risk,
		authorization: answer.authorization,
		decision: answer.decision,
		rationale: answer.rationale,
	})
}

/// The decision the core makes from one judgement.
///
/// A denial is a denial. An allow is honoured only for work whose risk the
/// policy lets a reviewer settle on its own, or where the reviewer found
/// that the user explicitly asked for this action. A critical class is
/// never settled here, whatever the reviewer said about it.
pub(crate) fn settle(verdict: &ReviewVerdict) -> ReviewDecision {
	match (verdict.decision, verdict.risk, verdict.authorization) {
		(ReviewDecision::Deny, _, _)
		| (ReviewDecision::Allow, ReviewRisk::Critical, _) => ReviewDecision::Deny,
		(
			ReviewDecision::Allow,
			ReviewRisk::High,
			ReviewAuthorization::Absent | ReviewAuthorization::Partial,
		) => ReviewDecision::Deny,
		(
			ReviewDecision::Allow,
			ReviewRisk::High,
			ReviewAuthorization::Sufficient,
		)
		| (ReviewDecision::Allow, ReviewRisk::Low | ReviewRisk::Medium, _) => {
			ReviewDecision::Allow
		}
	}
}
