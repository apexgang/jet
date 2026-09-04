use std::collections::HashMap;
use std::sync::Mutex;

use jet_store::ConversationPageKey;
use uuid::Uuid;

use crate::{EventSequence, PageCursor};

const PAGE_CURSOR_TTL_MS: i64 = 5 * 60 * 1_000;
const MAX_PAGE_CURSORS: usize = 1_024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConversationPageState {
	pub(crate) after: ConversationPageKey,
	pub(crate) snapshot_revision: EventSequence,
	pub(crate) expires_at_unix_ms: i64,
}

#[derive(Debug, Default)]
pub(crate) struct ConversationPages {
	cursors: Mutex<HashMap<PageCursor, ConversationPageState>>,
}

impl ConversationPages {
	pub(crate) fn first_deadline(&self, now_unix_ms: i64) -> i64 {
		now_unix_ms.saturating_add(PAGE_CURSOR_TTL_MS)
	}

	pub(crate) fn issue(
		&self,
		after: Option<ConversationPageKey>,
		snapshot_revision: EventSequence,
		expires_at_unix_ms: i64,
		now_unix_ms: i64,
	) -> Option<PageCursor> {
		let after = after?;
		let mut cursors = self
			.cursors
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		cursors.retain(|_, state| state.expires_at_unix_ms > now_unix_ms);
		if cursors.len() >= MAX_PAGE_CURSORS
			&& let Some(oldest) = cursors
				.iter()
				.min_by_key(|(_, state)| state.expires_at_unix_ms)
				.map(|(cursor, _)| *cursor)
		{
			cursors.remove(&oldest);
		}
		let cursor = PageCursor(Uuid::new_v4());
		cursors.insert(
			cursor,
			ConversationPageState {
				after,
				snapshot_revision,
				expires_at_unix_ms,
			},
		);
		Some(cursor)
	}

	pub(crate) fn resume(
		&self,
		cursor: &PageCursor,
		now_unix_ms: i64,
	) -> Option<ConversationPageState> {
		let mut cursors = self
			.cursors
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		let state = cursors.get(cursor).copied()?;
		if state.expires_at_unix_ms <= now_unix_ms {
			cursors.remove(cursor);
			None
		} else {
			Some(state)
		}
	}
}
