//! Sleep until a committed change, a schedule deadline, or work needing recovery.
use crate::{Core, CoreError};
use std::time::Duration;

/// How long to wait for `due_unix_ms`, at least one second: a deadline
/// already passed, or a full queue or bounded catch-up, retries then.
fn delay_until(due_unix_ms: i64, now_unix_ms: i64) -> Duration {
	Duration::from_millis(
		due_unix_ms.saturating_sub(now_unix_ms).max(1000) as u64
	)
}

impl Core {
	/// Wait without polling an idle Plane. Commands wake this before its earliest
	/// deadline; active executions retain bounded recovery and power revalidation.
	/// # Errors
	/// Returns a store error when the next deadline cannot be established.
	pub async fn wait_for_maintenance(&self) -> Result<(), CoreError> {
		// Notify retains a permit for changes committed during the preceding sweep.
		let changes = self.maintenance_work.notified();
		let delay = self
			.store
			.read(async |tx| {
				let active = !tx.active_execution_ids("").await?.is_empty()
					|| !tx.live_terminals("").await?.is_empty();
				let pending = !tx.pending_turn_queues("").await?.is_empty();
				let schedule = tx
					.next_schedule_deadline()
					.await?
					.map(|due| delay_until(due, self.now_unix_ms()));
				let trash = tx
					.next_trash_expiry()
					.await?
					.map(|due| delay_until(due, self.now_unix_ms()));
				// An approved Autodelete rule's next match is due when the
				// first Conversation not yet idle long enough becomes so.
				// Conversations already past the cutoff are the sweep's:
				// one it declines to stage sets no deadline, so a protected
				// match does not wake the Plane every second.
				let autodelete =
					crate::autodelete::next_deadline(tx, self.now_unix_ms())
						.await?
						.map(|due| delay_until(due, self.now_unix_ms()));
				// Usage rows leave their retention tier on their own clock
				// (ADR-0045).
				let usage = crate::usage::history::next_deadline(tx)
					.await?
					.map(|due| delay_until(due, self.now_unix_ms()));
				let recovery = if active {
					Some(Duration::from_secs(1))
				} else if pending {
					Some(Duration::from_secs(60))
				} else {
					None
				};
				Ok::<_, CoreError>(
					schedule
						.into_iter()
						.chain(recovery)
						.chain(trash)
						.chain(autodelete)
						.chain(usage)
						.min(),
				)
			})
			.await?;
		let retirement = match &self.run_host {
			Some(host) => host.craft_retirement_delay().await,
			None => None,
		};
		let deadline = delay.into_iter().chain(retirement).min();
		tokio::select! {
			_ = changes => {},
			_ = async {
				match deadline {
					Some(delay) => tokio::time::sleep(delay).await,
					None => std::future::pending().await,
				}
			} => {},
		}
		Ok(())
	}
}
