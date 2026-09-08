//! Version-aware Event journal pages.

use crate::event::{EVENT_PAGE_LIMIT, Event, EventPage, EventSequence};
use crate::{Core, CoreError, QueryResult};

pub(crate) async fn events(
	core: &Core,
	after: EventSequence,
) -> Result<QueryResult, CoreError> {
	events_visible_to(core, after, EventVisibility::Current).await
}

pub(crate) async fn legacy_events(
	core: &Core,
	after: EventSequence,
) -> Result<QueryResult, CoreError> {
	events_visible_to(core, after, EventVisibility::BeforeNames).await
}

#[derive(Clone, Copy)]
enum EventVisibility {
	Current,
	BeforeNames,
}

async fn events_visible_to(
	core: &Core,
	after: EventSequence,
	visibility: EventVisibility,
) -> Result<QueryResult, CoreError> {
	core.store
		.read(async |tx| {
			let (journal_cursor, records) = match visibility {
				EventVisibility::Current => {
					tx.events_after(after.0, EVENT_PAGE_LIMIT).await?
				}
				EventVisibility::BeforeNames => {
					tx.legacy_events_after(after.0, EVENT_PAGE_LIMIT).await?
				}
			};
			let mut bytes = 0;
			let events = records
				.into_iter()
				.take_while(|event| {
					// Room for envelope/identities after jetd translates the page.
					let size = event.payload.len() + 1024;
					if bytes != 0 && bytes + size > 512 * 1024 {
						return false;
					}
					bytes += size;
					true
				})
				.map(Event::try_from)
				.collect::<Result<Vec<_>, _>>()?;
			Ok(QueryResult::Events(EventPage {
				cursor: EventSequence(journal_cursor),
				events,
			}))
		})
		.await
}
