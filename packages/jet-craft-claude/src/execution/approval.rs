//! The in-process MCP server the Harness asks before using a tool.
//!
//! Claude Code routes a permission request to the tool named by
//! `--permission-prompt-tool`, and reaches a server declared in `initialize`
//! by sending its JSON-RPC messages back out as `mcp_message` requests. Server
//! plumbing is answered here; only a call of the permission tool is a decision,
//! and that one belongs to Jet, never to this Craft.
use crate::execution::harness;
use jet_protocol::CraftApprovalDecision;
use serde_json::{Value, json};

/// What one native event addressed to this Craft's server turns into.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Served {
	/// The native line answering the Harness; nothing here is a decision.
	Reply(String),
	/// The Harness is asking whether it may use a tool. Only Jet answers.
	Asking(Request),
	/// Not addressed to this Craft's server.
	Ignored,
}

/// One unanswered permission request, held until Jet decides. It is carried
/// in the Run's checkpoint, so a Craft that restarts while the Harness waits
/// can still deliver the decision instead of leaving it blocked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Request {
	/// The identity Jet answers with, and the one this Craft replies to.
	pub(crate) id: String,
	/// Native tool the Harness asked to use.
	pub(crate) tool: String,
	/// JSON-RPC correlation inside the held request.
	call: Value,
	/// Native tool input, returned unchanged when the decision allows it.
	input: Value,
}

/// Read one native event as a message for this Craft's server.
pub(crate) fn served(event: &Value) -> Served {
	if event.get("type").and_then(Value::as_str) != Some("control_request") {
		return Served::Ignored;
	}
	let request = &event["request"];
	if request.get("subtype").and_then(Value::as_str) != Some("mcp_message")
		|| request.get("server_name").and_then(Value::as_str)
			!= Some(harness::SERVER)
	{
		return Served::Ignored;
	}
	let Some(id) = event.get("request_id").and_then(Value::as_str) else {
		return Served::Ignored;
	};
	let message = &request["message"];
	let call = message.get("id").cloned().unwrap_or(Value::Null);
	match message.get("method").and_then(Value::as_str) {
		Some("initialize") => Served::Reply(reply(
			id,
			&call,
			&json!({
				"protocolVersion": "2025-06-18",
				"capabilities": {"tools": {}},
				"serverInfo": {"name": harness::SERVER, "version": "1"},
			}),
		)),
		Some("tools/list") => Served::Reply(reply(id, &call, &tools())),
		Some("tools/call")
			if message.pointer("/params/name").and_then(Value::as_str)
				== Some(harness::TOOL) =>
		{
			Served::Asking(Request {
				id: id.to_owned(),
				tool: message
					.pointer("/params/arguments/tool_name")
					.and_then(Value::as_str)
					.unwrap_or(harness::TOOL)
					.to_owned(),
				call,
				input: message
					.pointer("/params/arguments/input")
					.cloned()
					.unwrap_or_else(|| json!({})),
			})
		}
		// A notification has no reply of its own, and an unknown method is
		// still answered so the Harness is never left waiting on this Craft.
		Some(_) | None => Served::Reply(reply(id, &call, &json!({}))),
	}
}

impl Request {
	/// Describe the held request for the host, which shows it to a person or
	/// a reviewer. The input travels as it arrived, bounded but never
	/// rewritten: what is decided about has to be what was asked for.
	pub(crate) fn asked(&self) -> jet_protocol::CraftApprovalRequest {
		jet_protocol::CraftApprovalRequest {
			request_id: self.id.clone(),
			tool: self.tool.clone(),
			action: jet_craft_sdk::approval_action(&self.input),
		}
	}

	/// Turn Jet's decision into the answer the Harness waits for. An allowed
	/// call keeps its original input: this Craft never edits work it merely
	/// carried, and an approval is for exactly what was shown.
	pub(crate) fn decided(&self, decision: CraftApprovalDecision) -> String {
		let outcome = match decision {
			CraftApprovalDecision::AllowOnce => {
				json!({"behavior": "allow", "updatedInput": self.input})
			}
			CraftApprovalDecision::Deny => {
				json!({"behavior": "deny", "message": "Denied through Jet"})
			}
		};
		reply(
			&self.id,
			&self.call,
			&json!({
				"content": [{"type": "text", "text": outcome.to_string()}],
			}),
		)
	}
}

pub(crate) fn tools() -> Value {
	json!({
		"tools": [{
			"name": harness::TOOL,
			"description": "Ask Jet whether this tool may be used.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"tool_name": {"type": "string"},
					"input": {"type": "object"},
					"tool_use_id": {"type": "string"},
				},
				"required": ["tool_name", "input"],
			},
		}],
	})
}

/// One answered JSON-RPC message, as the native line that delivers it.
pub(crate) fn reply(request_id: &str, call: &Value, result: &Value) -> String {
	harness::control_response(
		request_id,
		json!({
			"mcp_response": {"jsonrpc": "2.0", "id": call, "result": result},
		}),
	)
}

#[cfg(test)]
mod approval_tests {
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
			result(served(&message("initialize", 0, json!({}))))["serverInfo"]
				["name"],
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
			denied["response"]["response"]["mcp_response"]["result"]["content"]
				[0]["text"]
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
}
