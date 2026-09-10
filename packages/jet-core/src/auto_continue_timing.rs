//! Absolute quota resets and refreshes of already admitted retry input.
use super::*;

pub(super) fn reset_at(condition: &Condition) -> Option<i64> {
	condition
		.usage
		.resets_in_seconds
		.and_then(|s| s.checked_mul(1000))
		.and_then(|ms| i64::try_from(ms).ok())
		.map(|ms| condition.observed_at.saturating_add(ms))
}
pub(super) fn due_at(condition: &Condition, fallback: u64) -> i64 {
	reset_at(condition).unwrap_or_else(|| {
		condition.observed_at.saturating_add(fallback as i64)
	})
}
pub(super) async fn refresh_pending(
	tx: &mut WriteTransaction,
	id: ConversationId,
	queue: &mut turn_queue::Queue,
	conditions: &[Condition],
	client_id: crate::ClientId,
	now: i64,
) -> Result<(), CoreError> {
	let Some(previous) = queue.auto_continue.as_ref() else {
		return Ok(());
	};
	if queue.quota_until.is_none() {
		return Ok(());
	}
	// Admission already fixed the fallback deadline. Repeated reports with
	// no reset cannot extend that deadline indefinitely.

	let Some(condition) = conditions
		.iter()
		.filter(|c| {
			c.turn_id == previous.triggering_turn
				&& exhausted(&c.usage)
				&& reset_at(c).is_some()
		})
		.max_by_key(|c| reset_at(c))
	else {
		return Ok(());
	};
	let due = reset_at(condition).expect("provider reset checked");
	if due <= previous.due_at_unix_ms && previous.due_at_unix_ms != i64::MAX {
		return Ok(());
	}
	let mut retry = previous.clone();
	retry.usage = condition.usage.clone();
	retry.observed_at_unix_ms = condition.observed_at;
	retry.due_at_unix_ms = due;
	queue.quota_until = Some(due);
	queue.auto_continue = Some(retry.clone());
	turn_queue::save(tx, id, queue).await?;
	changed(tx, &Actor::InteractiveClient { client_id }, id, retry, now).await
}
