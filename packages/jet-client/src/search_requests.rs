//! Searching one Plane's human-visible Conversation content (ADR-0036).

use jet_protocol::{QueryRequest, QueryResponse, SearchResult};

use crate::connection::{Client, ClientError};
use crate::requests::unexpected;

impl Client {
	/// Finds the Conversation content on this Plane that contains every
	/// whitespace-separated term of `text`: at most 64 hits, best match
	/// first, each naming its Conversation and the Event that carried it.
	/// A GUI connected to several Planes asks each and merges the
	/// answers. Needs protocol minor 12.
	///
	/// # Errors
	///
	/// Returns [`ClientError::FeatureUnavailable`] when the negotiated
	/// minor predates search, [`ClientError::Remote`] when the daemon
	/// reports a stable error such as `search.empty`,
	/// `search.text_too_long`, or `search.too_many_terms`, or the
	/// transport failure otherwise.
	pub async fn search(
		&self,
		text: &str,
	) -> Result<SearchResult, ClientError> {
		self.require_minor(jet_protocol::SEARCH_MINOR)?;
		match self
			.query(QueryRequest::Search { text: text.into() })
			.await?
		{
			QueryResponse::Search(result) => Ok(result),
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
			| QueryResponse::ProjectEntry(_)
			| QueryResponse::PromotionPreview(_)
			| QueryResponse::RunExecution(_)) => Err(unexpected(&other)),
		}
	}
}
