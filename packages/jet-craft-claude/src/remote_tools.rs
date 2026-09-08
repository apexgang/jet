//! Native MCP remote tool forwarding; destination authority stays in jetd.
use jet_craft_sdk::CraftError;
use jet_protocol::{CraftRemoteTool, NoVisaSelection, RemoteToolOutcome};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) enum Observed {
	Reply(String),
	Call(CraftRemoteTool),
}
pub(crate) struct RemoteTools {
	selection: NoVisaSelection,
	pending: HashMap<Uuid, (String, Value)>,
}
impl RemoteTools {
	pub(crate) fn new(selection: NoVisaSelection) -> Self {
		Self {
			selection,
			pending: HashMap::new(),
		}
	}
	pub(crate) fn pending(&self) -> bool {
		!self.pending.is_empty()
	}
	pub(crate) fn observe(&mut self, event: &Value) -> Option<Observed> {
		if event["type"] != "control_request"
			|| event["request"]["subtype"] != "mcp_message"
			|| event["request"]["server_name"] != crate::harness::SERVER
		{
			return None;
		}
		let id = event["request_id"].as_str()?;
		let message = &event["request"]["message"];
		let call = message.get("id").cloned().unwrap_or(Value::Null);
		if message["method"] == "tools/list" {
			let mut tools = crate::approval::tools();
			tools["tools"].as_array_mut()?.push(json!({
                "name":"remote", "description":format!("Use bounded Jet tools on these selected destinations: {}. Native processes stay on the origin. Reuse operation_id only when retrying the exact same action. Approval-required means no work ran; request destination review before retrying. Never retry a mutation with a new identity after an uncertain result.", serde_json::to_string(&self.selection).ok()?),
                "inputSchema":serde_json::from_str::<Value>(include_str!("../../jet-protocol/contracts/remote-tool.schema.json")).ok()?,
            }));
			return Some(Observed::Reply(crate::approval::reply(
				id, &call, &tools,
			)));
		}
		if message["method"] != "tools/call"
			|| message["params"]["name"] != "remote"
		{
			return None;
		}
		let arguments = &message["params"]["arguments"];
		let request =
			serde_json::from_value::<CraftRemoteTool>(arguments.clone());
		let Ok(request) = request else {
			return Some(Observed::Reply(invalid(id, &call)));
		};
		if id.len() > 256
			|| self.pending.len() >= 16
			|| self.pending.contains_key(&request.operation_id)
		{
			return Some(Observed::Reply(invalid(id, &call)));
		}
		self.pending.insert(request.operation_id, (id.into(), call));
		Some(Observed::Call(request))
	}
	pub(crate) fn reply(
		&mut self,
		operation_id: Uuid,
		outcome: RemoteToolOutcome,
	) -> Result<String, CraftError> {
		let (id, call) = self
			.pending
			.remove(&operation_id)
			.ok_or(CraftError::InvalidMessage)?;
		Ok(crate::approval::reply(
			&id,
			&call,
			&json!({
				"isError":matches!(outcome, RemoteToolOutcome::Failed { .. }),
				"content":[{"type":"text","text":serde_json::to_string(&outcome).map_err(|_| CraftError::InvalidMessage)?}],
			}),
		))
	}
}
fn invalid(id: &str, call: &Value) -> String {
	crate::approval::reply(
		id,
		call,
		&json!({"isError":true,"content":[{"type":"text","text":"Invalid or duplicate bounded remote tool request"}]}),
	)
}
