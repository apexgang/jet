use super::{Served, served};
use jet_protocol::CraftApprovalDecision;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

fn message(method: &str, id: u32, params: Value) -> Value {
	json!({
		"type": "control_request", "request_id": "req-1",
		"request": {
			"subtype": "mcp_message", "server_name": "jet",
			"message": {"jsonrpc": "2.0", "id": id, "method": method, "params": params},
		},
	})
}

/// The reply line's JSON-RPC result, so a test reads what the Harness reads.
fn result(served: Served) -> Value {
	let Served::Reply(line) = served else {
		panic!("expected a reply, got {served:?}")
	};
	let reply: Value = serde_json::from_str(line.trim_end()).unwrap();
	reply["response"]["response"]["mcp_response"]["result"].clone()
}

#[test]
fn only_this_crafts_own_server_is_answered() {
	assert_eq!(served(&json!({"type": "assistant"})), Served::Ignored);
	let mut elsewhere = message("tools/list", 1, json!({}));
	elsewhere["request"]["server_name"] = json!("other");
	assert_eq!(served(&elsewhere), Served::Ignored);
	let mut unnamed = message("tools/list", 1, json!({}));
	unnamed["request_id"] = Value::Null;
	assert_eq!(served(&unnamed), Served::Ignored);
}

#[test]
fn server_plumbing_is_answered_without_asking_anyone() {
	assert_eq!(
		result(served(&message("initialize", 0, json!({}))))["serverInfo"]["name"],
		json!("jet")
	);
	assert_eq!(
		result(served(&message("tools/list", 1, json!({}))))["tools"][0]["name"],
		json!("approve")
	);
	// A notification, and a method this Craft does not know, still leave the
	// Harness with an answer rather than waiting on a server that is here.
	assert_eq!(
		result(served(&message("notifications/initialized", 0, json!({})))),
		json!({})
	);
	assert_eq!(
		result(served(&message("tools/unknown", 2, json!({})))),
		json!({})
	);
}

#[test]
fn a_permission_call_is_a_decision_only_jet_makes() {
	let call = message(
		"tools/call",
		2,
		json!({
			"name": "approve",
			"arguments": {
				"tool_name": "Write",
				"input": {"file_path": "note.txt", "content": "text"},
				"tool_use_id": "toolu_1",
			},
		}),
	);
	let Served::Asking(request) = served(&call) else {
		panic!("a permission call needs Jet")
	};
	assert_eq!(request.id, "req-1");

	// Allowing returns exactly the input that was shown, never an edited one.
	let allowed: Value = serde_json::from_str(
		request.decided(CraftApprovalDecision::AllowOnce).trim_end(),
	)
	.unwrap();
	let decision: Value = serde_json::from_str(
		allowed["response"]["response"]["mcp_response"]["result"]["content"][0]
			["text"]
			.as_str()
			.unwrap(),
	)
	.unwrap();
	assert_eq!(
		decision,
		json!({
			"behavior": "allow",
			"updatedInput": {"file_path": "note.txt", "content": "text"},
		})
	);

	let denied: Value = serde_json::from_str(
		request.decided(CraftApprovalDecision::Deny).trim_end(),
	)
	.unwrap();
	let decision: Value = serde_json::from_str(
		denied["response"]["response"]["mcp_response"]["result"]["content"][0]
			["text"]
			.as_str()
			.unwrap(),
	)
	.unwrap();
	assert_eq!(
		decision,
		json!({
			"behavior": "deny", "message": "Denied through Jet",
		})
	);
}

/// A call that names another tool on this server is not a permission
/// decision, so it is answered rather than held.
#[test]
fn a_call_of_another_tool_is_not_an_approval() {
	assert_eq!(
		result(served(&message(
			"tools/call",
			3,
			json!({"name": "something-else", "arguments": {}}),
		))),
		json!({})
	);
}
