//! Opaque, expiring keyset cursors bind diff pages to their query and Git state.
use crate::{ChangeDiff, DiffScope, EventSequence, PageCursor, RunId};
use std::{collections::HashMap, sync::Mutex};

#[derive(Debug, Clone)]
pub(crate) struct PageState {
	pub(crate) run_id: RunId,
	pub(crate) scope: DiffScope,
	pub(crate) after_path: String,
	pub(crate) before: [u8; 32],
	pub(crate) after: [u8; 32],
	pub(crate) revision: EventSequence,
	pub(crate) files_hash: [u8; 32],
	pub(crate) latest_turn: u32,
	pub(crate) expires: i64,
}

#[derive(Debug, Default)]
pub(crate) struct Pages(Mutex<HashMap<PageCursor, PageState>>);

pub(crate) enum Start {
	First,
	After(Box<PageState>),
}

impl Pages {
	pub(crate) fn resume(
		&self,
		cursor: PageCursor,
		now: i64,
	) -> Option<PageState> {
		let mut pages = self
			.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		pages.retain(|_, page| page.expires > now);
		pages.get(&cursor).cloned()
	}

	pub(crate) fn issue(
		&self,
		diff: &ChangeDiff,
		files_hash: [u8; 32],
		expires: i64,
		now: i64,
	) -> Option<PageCursor> {
		let after_path = diff.files.last()?.path.clone();
		let mut pages = self
			.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		pages.retain(|_, page| page.expires > now);
		if pages.len() >= 1024
			&& let Some(oldest) = pages
				.iter()
				.min_by_key(|(_, page)| page.expires)
				.map(|(cursor, _)| *cursor)
		{
			pages.remove(&oldest);
		}
		let cursor = PageCursor(uuid::Uuid::new_v4());
		pages.insert(
			cursor,
			PageState {
				run_id: diff.run_id,
				scope: diff.scope.clone(),
				after_path,
				before: fingerprint(&diff.before),
				after: fingerprint(&diff.after),
				revision: diff.cursor,
				files_hash,
				latest_turn: diff.latest_turn,
				expires,
			},
		);
		Some(cursor)
	}
}

pub(crate) fn bound_page(
	core: &crate::Core,
	diff: &mut ChangeDiff,
	start: Start,
) -> Result<(), crate::CoreError> {
	diff.files.sort_by(|a, b| a.path.cmp(&b.path));
	let files_hash = fingerprint(&diff.files);
	let now = core.now_unix_ms();
	let (after_path, expires) = match start {
		Start::First => (None, now.saturating_add(5 * 60 * 1000)),
		Start::After(page) => {
			if page.expires <= now
				|| page.files_hash != files_hash
				|| page.before != fingerprint(&diff.before)
				|| page.after != fingerprint(&diff.after)
			{
				return Err(crate::CoreError::pagination_stale(diff.cursor));
			}
			{
				diff.cursor = page.revision;
				diff.latest_turn = page.latest_turn;
				(Some(page.after_path), page.expires)
			}
		}
	};
	diff.total_files = u32::try_from(diff.files.len())
		.map_err(crate::checkpoint::change_artifact::failed)?;
	let first = after_path.map_or(0, |path| {
		diff.files.partition_point(|file| file.path <= path)
	});
	// ASVS 2.2.1: both count and encoded-byte budgets bound one control reply.
	let mut bytes = 0;
	diff.files = std::mem::take(&mut diff.files)
		.into_iter()
		.skip(first)
		.take(128)
		.take_while(|file| {
			bytes += serde_json::to_vec(file).map_or(usize::MAX, |v| v.len());
			bytes <= 128 * 1024
		})
		.collect();
	if first + diff.files.len() < diff.total_files as usize {
		diff.next_page =
			core.checkpoint_pages.issue(diff, files_hash, expires, now);
	}
	Ok(())
}

fn fingerprint(value: &impl serde::Serialize) -> [u8; 32] {
	use sha2::{Digest, Sha256};
	Sha256::digest(serde_json::to_vec(value).expect("file metadata serializes"))
		.into()
}
