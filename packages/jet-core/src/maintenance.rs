//! Sleep until a committed change, a schedule deadline, or work needing recovery.
use crate::{Core, CoreError};
use std::time::Duration;

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
				let schedule = tx.next_schedule_deadline().await?.map(|due| {
					// A full queue or bounded catch-up waits one second before retrying.
					Duration::from_millis(
						due.saturating_sub(self.now_unix_ms()).max(1000) as u64,
					)
				});
				let recovery = if active {
					Some(Duration::from_secs(1))
				} else if pending {
					Some(Duration::from_secs(60))
				} else {
					None
				};
				Ok::<_, CoreError>(schedule.into_iter().chain(recovery).min())
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
