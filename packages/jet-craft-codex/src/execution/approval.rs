//! Native app-server approvals held for an explicit Jet decision.
use jet_protocol::CraftApprovalDecision;
use serde_json::{Value, json};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Request {
	pub(crate) id: String,
	/// Native approval kind the app-server asked about.
	tool: String,
	/// The parameters of that request, as they arrived.
	action: Value,
	call: Value,
}

pub(crate) fn request(event: &Value, thread: Option<&str>) -> Option<Request> {
	let method = event.get("method")?.as_str()?;
	if !matches!(
		method,
		"item/commandExecution/requestApproval"
			| "item/fileChange/requestApproval"
			| "item/permissions/requestApproval"
	) || event.pointer("/params/threadId")?.as_str()? != thread?
	{
		return None;
	}
	let call = event.get("id")?.clone();
	let id = match &call {
		Value::String(id) if !id.is_empty() => id.clone(),
		Value::Number(id) => id.to_string(),
		_ => return None,
	};
	Some(Request {
		id,
		tool: method.into(),
		action: event.get("params").cloned().unwrap_or(Value::Null),
		call,
	})
}

impl Request {
	/// Describe the held request for the host, which shows it to a person or
	/// a reviewer. The parameters travel as they arrived, bounded but never
	/// rewritten.
	pub(crate) fn asked(&self) -> jet_protocol::CraftApprovalRequest {
		jet_protocol::CraftApprovalRequest {
			request_id: self.id.clone(),
			tool: self.tool.clone(),
			action: jet_craft_sdk::approval_action(&self.action),
		}
	}

	pub(crate) fn decided(&self, decision: CraftApprovalDecision) -> String {
		let decision = match decision {
			CraftApprovalDecision::AllowOnce => "accept",
			CraftApprovalDecision::Deny => "decline",
		};
		format!(
			"{}\n",
			json!({"id": self.call, "result": {"decision": decision}}),
		)
	}
}
