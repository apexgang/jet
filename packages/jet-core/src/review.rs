//! Automatic review of Harness approval requests (ADR-0012).
//!
//! Automatic review answers only what would otherwise have to wait for a
//! person. It changes nothing else: the Harness keeps the sandbox,
//! filesystem, network, tool, and permission boundary it already had, and
//! the single answer Jet sends back is for exactly the request that was
//! asked. There is no blanket approval to grant and none to inherit.
//!
//! Everything the reviewer returns is data. The trusted core validates it,
//! decides what the judgement is allowed to authorize, and records the
//! decision it actually made.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AccountBinding, AccountBindingId, CoreError, ProviderId, RunId};

/// Longest native approval identity this core will carry.
pub(crate) const REQUEST_ID_BYTES: usize = 128;
/// Longest native tool or permission name.
pub(crate) const TOOL_BYTES: usize = 128;
/// Longest exact requested action.
pub(crate) const ACTION_BYTES: usize = 4096;
/// Longest visible transcript one reviewer is shown.
pub(crate) const TRANSCRIPT_BYTES: usize = 12 * 1024;
/// Longest rationale a reviewer may return.
pub(crate) const RATIONALE_BYTES: usize = 1024;

/// One native approval request a Craft is holding, as the Harness asked
/// for it. The action is Conversation content, never an instruction to
/// anything in Jet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
	/// Native approval identity, answered exactly once.
	pub request_id: String,
	/// Native tool or permission name.
	pub tool: String,
	/// The exact action the Harness asked to carry out.
	pub action: String,
}

impl ApprovalRequest {
	/// Refuses a request whose identity or content is outside its bounds.
	///
	/// # Errors
	/// Returns an `invalid_input` [`CoreError`]; nothing about the Run
	/// changes and the Craft keeps holding the native request.
	pub(crate) fn validate(&self) -> Result<(), CoreError> {
		if self.request_id.is_empty()
			|| self.request_id.len() > REQUEST_ID_BYTES
			|| self.request_id.chars().any(char::is_control)
			|| self.tool.is_empty()
			|| self.tool.len() > TOOL_BYTES
			|| self.tool.chars().any(char::is_control)
			|| self.action.len() > ACTION_BYTES
		{
			return Err(CoreError::invalid_input(
				"review.request_invalid",
				"an approval request names a bounded native identity, tool, \
				 and action",
			));
		}
		Ok(())
	}
}

/// How much the requested action could cost, as the reviewer judged it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRisk {
	/// Ordinary work inside what the Harness may already do.
	Low,
	/// Work worth naming, whose effects stay recoverable.
	Medium,
	/// Work a person would want to have asked for.
	High,
	/// A class Jet does not let a reviewer authorize.
	Critical,
}

/// Whether the visible transcript shows the user asking for this action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAuthorization {
	/// Nothing in the transcript asks for it.
	Absent,
	/// The transcript asks for something adjacent, not for this.
	Partial,
	/// The user explicitly asked for this action.
	Sufficient,
}

/// The only two answers one held request can be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
	/// Allow exactly this request, once.
	Allow,
	/// Deny exactly this request.
	Deny,
}

/// What a reviewer answered. It is a judgement, not an authorization: the
/// core decides separately what the judgement may authorize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVerdict {
	/// Judged risk of the exact requested action.
	pub risk: ReviewRisk,
	/// Judged user authorization for it in the visible transcript.
	pub authorization: ReviewAuthorization,
	/// What the reviewer would do.
	pub decision: ReviewDecision,
	/// Why, in the reviewer's own words.
	pub rationale: String,
}

/// Which reviewer answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reviewer {
	/// The Harness's own separate reviewer.
	Native,
	/// Jet's versioned equivalent, run through the selected binding.
	Equivalent,
}

/// The reviewer a Craft pinned before it saw any Conversation content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerSelection {
	/// Which reviewer will answer.
	pub reviewer: Reviewer,
	/// Exact Model identity, never an alias resolved after selection.
	pub model: String,
	/// Bounded opaque host pin; never sent as model input.
	pub adapter_state: String,
}

/// Everything one reviewer is shown: the bounded visible transcript and
/// the exact requested action. There is nothing else in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewInput {
	/// Visible Conversation content, oldest first.
	pub transcript: String,
	/// Native tool or permission name.
	pub tool: String,
	/// The exact requested action.
	pub action: String,
}

/// Untrusted response to one review request.
#[derive(Debug)]
pub struct ReviewReply {
	/// Actual Model reported by the reviewer, checked against the selection.
	pub model: String,
	/// Which reviewer produced the judgement.
	pub reviewer: Reviewer,
	/// At most 4096 bytes; hosts must bound reads before allocating them.
	pub output: Vec<u8>,
}

/// The exact policy one review ran under. Version 1 means one reviewer
/// request, no tools, a bounded transcript, and the exact requested action.
/// Version 2 adds trusted action eligibility, durable per-turn denial
/// limits, and single exact-action retries. A review only exists on a Plane that turned Automatic review on, so
/// there is no disabled state to record here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomaticReviewPolicy {
	/// Version of the limits and rules.
	pub version: u32,
	/// Persistent consent recorded for a reviewer outside the Run's own
	/// Provider.
	pub cross_provider_consent: bool,
}

/// What one review came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReviewOutcome {
	/// The trusted core refused the action independently of any reviewer.
	Denied {
		/// Stable, content-free policy rule.
		reason: String,
	},
	/// The reviewer answered and the core settled what it authorizes.
	Decided {
		/// The reviewer's own judgement.
		verdict: ReviewVerdict,
		/// The decision the core actually made and sent.
		decision: ReviewDecision,
	},
	/// No automatic decision was made, so the request still waits for a
	/// person. Nothing was allowed.
	Unavailable {
		/// Stable, content-free reason.
		reason: String,
	},
}

/// One recorded review and its complete routing and policy attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalReview {
	/// Plane-assigned review identity.
	pub review_id: Uuid,
	/// The Run whose Harness asked.
	pub run_id: RunId,
	/// The exact request that was reviewed.
	pub request: ApprovalRequest,
	/// Which reviewer answered; absent when none was reached.
	pub reviewer: Option<Reviewer>,
	/// Provider of the selected binding.
	pub provider: Option<ProviderId>,
	/// Explicit selected binding, including one that became unavailable.
	pub binding_id: Option<AccountBindingId>,
	/// Exact reviewer Model; absent when selection was unavailable.
	pub model: Option<String>,
	/// Policy resolved for this review and the consent it recorded.
	pub policy: AutomaticReviewPolicy,
	/// The judgement and decision, or a closed failure.
	pub outcome: ReviewOutcome,
}

/// Trusted Adapter for the reviewer of one Account binding's Craft.
///
/// Implementations select the Provider's fastest suitable low-effort
/// reviewer Model, resolve only this binding's credentials, and perform
/// exactly one review request with no tools and no access to the Run's
/// working tree. An error never authorizes another account, Provider,
/// Plane, or retry, and never stands in for a decision: a reviewer that
/// cannot answer leaves the request for the user. Response reads must be
/// bounded before allocation.
pub trait ReviewHost: std::fmt::Debug + Send + Sync {
	/// Select a reviewer without receiving any Conversation content. The
	/// pinned Craft is the Run's own; implementations may use its Harness's
	/// native reviewer or run Jet's equivalent through `binding`.
	fn select<'a>(
		&'a self,
		craft: &'a crate::PinnedCraft,
		binding: &'a AccountBinding,
	) -> crate::RunFuture<'a, Result<ReviewerSelection, CoreError>>;

	/// Review exactly one held request under the pinned selection.
	/// Credentials are transport authentication, never prompt content.
	/// Return a stable error without native diagnostics.
	fn review<'a>(
		&'a self,
		craft: &'a crate::PinnedCraft,
		binding: &'a AccountBinding,
		selection: &'a ReviewerSelection,
		input: &'a ReviewInput,
	) -> crate::RunFuture<'a, Result<ReviewReply, CoreError>>;
}

/// A review that did not happen. It is never an allowance: the request
/// stays held and a person still decides.
pub(crate) fn unavailable(code: &'static str) -> CoreError {
	CoreError::conflict(
		code,
		"Automatic review is unavailable under the recorded policy",
	)
}
