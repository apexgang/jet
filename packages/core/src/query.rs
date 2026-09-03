//! Queries: read-only snapshots. Each snapshot and its journal cursor are
//! read in one consistent transaction (ADR-0092).

use jet_store::ReadTransaction;

use crate::conversation::{
	ConversationId, ConversationList, ConversationSnapshot,
};
use crate::error::CoreError;
use crate::event::{EVENT_PAGE_LIMIT, Event, EventSequence};
use crate::status::PlaneStatus;
use crate::{Actor, CORE_VERSION, Core, PlaneId};

/// Read-only requests answered with a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
	/// Snapshot of the daemon's Plane status.
	Status,
	/// Every Conversation on the Plane.
	Conversations,
	/// One Conversation with all of its Runs.
	Conversation {
		/// The Conversation to read.
		conversation_id: ConversationId,
	},
	/// Up to [`EVENT_PAGE_LIMIT`] journal Events strictly after a position.
	Events {
		/// The position to resume after; [`EventSequence::ORIGIN`] for all.
		after: EventSequence,
	},
}

/// Snapshots returned by [`Core::query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
	/// Snapshot of the daemon's Plane status.
	Status(PlaneStatus),
	/// Every Conversation on the Plane.
	Conversations(ConversationList),
	/// One Conversation with all of its Runs.
	Conversation(ConversationSnapshot),
	/// One page of journal Events in sequence order.
	Events(Vec<Event>),
}

impl Core {
	/// Runs `query` on behalf of `actor` and returns its snapshot.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] when the Actor is not authorized, the
	/// addressed entity does not exist, or the store cannot answer.
	pub fn query(
		&self,
		actor: &Actor,
		query: Query,
	) -> Result<QueryResult, CoreError> {
		actor.authorize()?;
		match query {
			Query::Status => {
				let plane = self.store.plane()?;
				Ok(QueryResult::Status(PlaneStatus {
					plane_id: PlaneId(plane.plane_id),
					daemon_starts: plane.daemon_starts,
					started_at: self.started_at,
					core_version: CORE_VERSION,
				}))
			}
			Query::Conversations => self.store.read(|tx| {
				Ok(QueryResult::Conversations(ConversationList {
					cursor: EventSequence(tx.event_cursor()?),
					conversations: tx
						.conversations()?
						.into_iter()
						.map(Into::into)
						.collect(),
				}))
			}),
			Query::Conversation { conversation_id } => {
				self.store.read(|tx| conversation(tx, conversation_id))
			}
			Query::Events { after } => self.store.read(|tx| {
				let events = tx.events_after(after.0, EVENT_PAGE_LIMIT)?;
				let events: Vec<Event> = events
					.into_iter()
					.map(Event::try_from)
					.collect::<Result<_, _>>()?;
				Ok(QueryResult::Events(events))
			}),
		}
	}
}

fn conversation(
	tx: &ReadTransaction<'_>,
	conversation_id: ConversationId,
) -> Result<QueryResult, CoreError> {
	let Some(record) = tx.conversation(conversation_id.0)? else {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	};
	Ok(QueryResult::Conversation(ConversationSnapshot {
		cursor: EventSequence(tx.event_cursor()?),
		conversation: record.into(),
		runs: tx
			.runs(conversation_id.0)?
			.into_iter()
			.map(Into::into)
			.collect(),
	}))
}
