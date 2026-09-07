//! Public Run snapshots and paged Event assertions shared by conformance tests.
use crate::support;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
pub async fn wait_for(
	wire: &mut support::RawConnection,
	run_id: &str,
	state: &str,
) -> Value {
	loop {
		wire.send(&json!({"kind":"query","id":2,"query":{"type":"run_execution","run_id":run_id}})).await;
		let response: Value = wire.receive().await;
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
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
}

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
