//! Native app-server approvals held for an explicit Jet decision.
use jet_protocol::CraftApprovalDecision;
use serde_json::{Value, json};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Request {
	pub(crate) id: String,
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
	Some(Request { id, call })
}

impl Request {
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
