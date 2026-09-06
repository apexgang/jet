//! Workspace promotion: previewing what applying a Workspace's changes
//! to a permanent checkout or branch of its Project would do, and
//! confirming exactly that (ADR-0025).

use jet_protocol::{
	CommandRequest, CommandResponse, PromotionBinding, PromotionDestination,
	PromotionPreview, QueryRequest, QueryResponse, WorkspacePromotion,
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

	/// Promotes a Workspace exactly as `binding`, taken from a preview,
	/// showed, under the Command identity `command_id`, which a retry must
	/// reuse (ADR-0093). The Plane computes the preview again first and
	/// refuses one the Workspace or the destination has moved past. The
	/// answer is the promotion as recorded: applying, with the durable
	/// Effect that applies it committed, or conflicted, with the paths that
	/// keep it from being applied; the Conversation snapshot shows where it
	/// stands afterwards. Needs protocol minor 11.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates promotion, [`ClientError::Remote`] when the daemon
	/// reports a stable error such as `workspace.promotion_stale`,
	/// `workspace.promotion_unbound`, `workspace.promotion_empty`, or
	/// `workspace.promotion_in_progress`, or the transport failure
	/// otherwise.
	pub async fn promote_workspace(
		&self,
		command_id: Uuid,
		binding: PromotionBinding,
	) -> Result<WorkspacePromotion, ClientError> {
		self.require_minor(jet_protocol::WORKSPACE_PROMOTION_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::PromoteWorkspace { binding },
			)
			.await?
		{
			CommandResponse::WorkspacePromotionRecorded(promotion) => {
				Ok(promotion)
			}
			other @ (CommandResponse::ConversationCreated(_)
			| CommandResponse::RunCreated(_)
			| CommandResponse::RunTransitioned(_)
			| CommandResponse::SettingSet { .. }
			| CommandResponse::SettingCleared { .. }
			| CommandResponse::AccountBound(_)
			| CommandResponse::AccountUnbound { .. }
			| CommandResponse::AuditEpochBegun { .. }
			| CommandResponse::PairingGateSet { .. }
			| CommandResponse::PairingOpened { .. }
			| CommandResponse::PairingClaimed { .. }
			| CommandResponse::PairingConfirmed { .. }
			| CommandResponse::PairingCompleted { .. }
			| CommandResponse::PairedClientAccessSet { .. }
			| CommandResponse::PairedClientRevoked { .. }
			| CommandResponse::ProjectRegistered(_)) => Err(unexpected(&other)),
		}
	}
}
