//! Queries: read-only snapshots. Each snapshot and its journal cursor are
//! read in one consistent transaction (ADR-0092).

use jet_store::{ConversationPageStart, ReadTransaction};

use crate::conversation::{
	ConversationId, ConversationList, ConversationSnapshot, PageCursor,
};
use crate::error::CoreError;
use crate::event::{EVENT_PAGE_LIMIT, Event, EventPage, EventSequence};
use crate::status::PlaneStatus;
use crate::{Actor, CORE_VERSION, Core, PlaneId};

/// Read-only requests answered with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
	/// Snapshot of the daemon's Plane status.
	Status,
	/// First bounded page of Conversations on the Plane.
	Conversations,
	/// Legacy minor-0 snapshot containing every Conversation in one result.
	LegacyConversations,
	/// Continue a fenced Conversation keyset snapshot.
	NextConversations {
		/// Opaque token returned by the previous page.
		cursor: PageCursor,
	},
	/// One Conversation with all of its Runs.
	Conversation {
		/// The Conversation to read.
		conversation_id: ConversationId,
	},
	/// One page of journal Events strictly after a position, with the
	/// journal cursor the page was read at.
	Events {
		/// The position to resume after; zero for the whole journal.
		after: EventSequence,
	},
}

/// Snapshots returned by [`Core::query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
	/// Snapshot of the daemon's Plane status.
	Status(PlaneStatus),
	/// One page of Conversations on the Plane.
	Conversations(ConversationList),
	/// One Conversation with all of its Runs.
	Conversation(ConversationSnapshot),
	/// One page of journal Events in sequence order.
	Events(EventPage),
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
			Query::Status => self.store.read(|tx| {
				let plane = tx.plane()?;
				Ok(QueryResult::Status(PlaneStatus {
					cursor: EventSequence(tx.event_cursor()?),
					plane_id: PlaneId(plane.plane_id),
					daemon_starts: plane.daemon_starts,
					started_at: self.started_at,
					core_version: CORE_VERSION,
				}))
			}),
			Query::Conversations => first_conversations(self),
			Query::LegacyConversations => self.store.read(|tx| {
				Ok(QueryResult::Conversations(ConversationList {
					cursor: EventSequence(tx.event_cursor()?),
					conversations: tx
						.conversations()?
						.into_iter()
						.map(Into::into)
						.collect(),
					next_page: None,
				}))
			}),
			Query::NextConversations { cursor } => {
				next_conversations(self, &cursor)
			}
			Query::Conversation { conversation_id } => {
				self.store.read(|tx| conversation(tx, conversation_id))
			}
			Query::Events { after } => self.store.read(|tx| {
				let (cursor, events) =
					tx.events_after(after.0, EVENT_PAGE_LIMIT)?;
				Ok(QueryResult::Events(EventPage {
					cursor: EventSequence(cursor),
					events: events
						.into_iter()
						.map(Event::try_from)
						.collect::<Result<_, _>>()?,
				}))
			}),
		}
	}
}

fn first_conversations(core: &Core) -> Result<QueryResult, CoreError> {
	let now = core.now_unix_ms();
	// ASVS 2.3.3 and 15.4.2: the projection page and its Event fence are
	// read atomically from one SQLite snapshot.
	let (cursor, (conversations, next)) = core.store.read(|tx| {
		Ok::<_, CoreError>((
			EventSequence(tx.event_cursor()?),
			tx.conversation_page(ConversationPageStart::First)?,
		))
	})?;
	let deadline = core.conversation_pages.first_deadline(now);
	let next_page = core.conversation_pages.issue(next, cursor, deadline, now);
	Ok(QueryResult::Conversations(ConversationList {
		cursor,
		conversations: conversations.into_iter().map(Into::into).collect(),
		next_page,
	}))
}

fn next_conversations(
	core: &Core,
	cursor: &PageCursor,
) -> Result<QueryResult, CoreError> {
	let now = core.now_unix_ms();
	let Some(state) = core.conversation_pages.resume(cursor, now) else {
		let current = core
			.store
			.read(|tx| Ok::<_, CoreError>(EventSequence(tx.event_cursor()?)))?;
		return Err(CoreError::pagination_stale(current));
	};
	let (current, (conversations, next)) = core.store.read(|tx| {
		Ok::<_, CoreError>((
			EventSequence(tx.event_cursor()?),
			tx.conversation_page(ConversationPageStart::After(state.after))?,
		))
	})?;
	if current != state.snapshot_revision {
		return Err(CoreError::pagination_stale(current));
	}
	let next_page = core.conversation_pages.issue(
		next,
		state.snapshot_revision,
		state.expires_at_unix_ms,
		now,
	);
	Ok(QueryResult::Conversations(ConversationList {
		cursor: state.snapshot_revision,
		conversations: conversations.into_iter().map(Into::into).collect(),
		next_page,
	}))
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
