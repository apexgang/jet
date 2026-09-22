//! Public Run snapshots and paged Event assertions shared by conformance tests.
#![allow(dead_code)]
use crate::support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// How long one Run may take to reach the state a test waits for. A step
/// that overruns it fails by name, with the Run's last snapshot, where no
/// shorter scenario-wide timeout says first that something stalled.
pub const STEP_BUDGET: Duration = Duration::from_secs(30);

/// Polls `run_id` until its activity or lifecycle is `state`, for at most
/// [`STEP_BUDGET`], and returns the snapshot that reached it. A daemon that stops
/// answering fails the step the same way, rather than hanging it.
pub async fn wait_for(
	wire: &mut support::RawConnection,
	run_id: &str,
	state: &str,
) -> Value {
	let deadline = Instant::now() + STEP_BUDGET;
	let mut last = Value::Null;
	loop {
		let remaining = deadline.saturating_duration_since(Instant::now());
		assert!(
			!remaining.is_zero(),
			"Run {run_id} did not reach {state} within {STEP_BUDGET:?}; last: {last}"
		);
		wire.send(&json!({"kind":"query","id":2,"query":{"type":"run_execution","run_id":run_id}})).await;
		let response: Value = match tokio::time::timeout(
			remaining,
			wire.receive(),
		)
		.await
		{
			Ok(response) => response,
			Err(_) => panic!(
				"Run {run_id} did not reach {state} within {STEP_BUDGET:?}: the daemon stopped answering; last: {last}"
			),
		};
		assert_eq!(response["kind"], "query_result", "{response}");
		let result = &response["result"];
		if result["activity"] == state || result["run"]["lifecycle"] == state {
			return result.clone();
		}
		assert!(
			!matches!(
				result["run"]["lifecycle"].as_str(),
				Some("failed" | "lost" | "canceled")
			),
			"Run ended before {state}: {result}"
		);
		last = result.clone();
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
}

#[allow(dead_code)]
pub async fn all_events(
	client: &jet_client::Client,
) -> jet_protocol::EventPage {
	let mut events = Vec::new();
	let mut after = 0;
	loop {
		let page = client.events_after(after).await.unwrap();
		if let Some(event) = page.events.last() {
			after = event.sequence;
		} else {
			assert_eq!(after, page.cursor, "page must make progress");
		}
		events.extend(page.events);
		if after == page.cursor {
			return jet_protocol::EventPage {
				cursor: after,
				events,
			};
		}
	}
}
