//! Workspace promotion: previewing what applying a Workspace's changes
//! to a permanent checkout or branch of its Project would do (ADR-0025).

use jet_protocol::{
	PromotionDestination, PromotionPreview, QueryRequest, QueryResponse,
};
use uuid::Uuid;

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

impl Client {
	/// Shows what promoting `workspace_id` to `destination` would do: the
	/// Workspace and destination as they stand, merged three ways against
	/// the Workspace base, with every path the merge cannot settle named
	/// rather than resolved. The answer binds what it compared and whom it
	/// was shown to. Nothing changes. Needs protocol minor 11.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates promotion, [`ClientError::Remote`] when the daemon
	/// reports a stable error such as `workspace.not_found`,
	/// `workspace.promotion_branch_not_found`, or
	/// `workspace.promotion_branch_checked_out`, or the transport failure
	/// otherwise.
	pub async fn preview_promotion(
		&self,
		workspace_id: Uuid,
		destination: PromotionDestination,
	) -> Result<PromotionPreview, ClientError> {
		self.require_minor(jet_protocol::WORKSPACE_PROMOTION_MINOR)?;
		match self
			.query(QueryRequest::PreviewPromotion {
				workspace_id,
				destination,
			})
			.await?
		{
			QueryResponse::PromotionPreview(preview) => Ok(preview),
			other @ (QueryResponse::Status(_)
			| QueryResponse::Conversations(_)
			| QueryResponse::Conversation(_)
			| QueryResponse::Events(_)
			| QueryResponse::Settings(_)
			| QueryResponse::Capabilities(_)
			| QueryResponse::AccountBindings(_)
			| QueryResponse::SecurityAudit(_)
			| QueryResponse::Pairing(_)
			| QueryResponse::Projects(_)
			| QueryResponse::ProjectPreview(_)
			| QueryResponse::ProjectEntry(_)) => Err(unexpected(&other)),
		}
	}
}
