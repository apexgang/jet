//! The Claude Code native protocol, which is structured throughout (ADR-0002).
//!
//! `claude --print --input-format stream-json --output-format stream-json`
//! reads newline-delimited JSON for as long as its standard input is open and
//! writes the same on its output. Every later turn is a user message written
//! there, and the process ends when that input closes, so nothing in this
//! Craft inspects a terminal.
use serde_json::{Value, json};
use uuid::Uuid;

/// The Claude Code releases this Craft is tested against (ADR-0104). A Harness
/// outside this range still runs, and the matrix in `docs/claude-code-craft.md`
/// records what was verified.
pub(crate) const TESTED_VERSIONS: &str = "2.1";

/// The in-process MCP server this Craft serves, and the one tool on it that
/// answers a permission request. The Harness addresses the tool by the name
/// it is given here, so the two must be declared together.
pub(crate) const SERVER: &str = "jet";
/// How the Harness names that tool once the server is registered.
pub(crate) const PERMISSION_TOOL: &str = "mcp__jet__approve";
/// The tool's own name inside the server.
pub(crate) const TOOL: &str = "approve";

/// The immutable argument vector for one Run. `--session-id` pins the native
/// Conversation identity before any output exists, so a resumable identity is
/// never inferred from a race with the Harness's first event.
pub(crate) fn arguments(session: Uuid, resume: Option<&str>) -> Vec<String> {
	let mut arguments: Vec<String> = [
		"--print",
		"--input-format",
		"stream-json",
		"--output-format",
		"stream-json",
		"--verbose",
		// Permission requests reach this Craft as calls on the SDK MCP
		// server it serves; without this they are denied unanswered.
		"--permission-prompt-tool",
		PERMISSION_TOOL,
	]
	.iter()
	.map(|argument| (*argument).to_owned())
	.collect();
	match resume {
		Some(native_conversation) => {
			arguments.push("--resume".into());
			arguments.push(native_conversation.to_owned());
		}
		None => {
			arguments.push("--session-id".into());
			arguments.push(session.to_string());
		}
	}
	arguments
}

/// Register this Craft's in-process MCP server before any turn runs. The
/// Harness then reaches the server by asking this Craft, over its own output
/// stream, rather than by connecting anywhere.
pub(crate) fn register_server() -> String {
	format!(
		"{}\n",
		json!({
			"type": "control_request",
			"request_id": Uuid::new_v4().to_string(),
			"request": {"subtype": "initialize", "sdkMcpServers": [SERVER]},
		})
	)
}

/// Answer one request the Harness made of this Craft.
pub(crate) fn control_response(request_id: &str, payload: Value) -> String {
	format!(
		"{}\n",
		json!({
			"type": "control_response",
			"response": {
				"subtype": "success",
				"request_id": request_id,
				"response": payload,
			},
		})
	)
}

/// One turn, as the native protocol's user message.
pub(crate) fn user_message(text: &str) -> String {
	format!(
		"{}\n",
		json!({
			"type": "user",
			"message": {"role": "user", "content": [{"type": "text", "text": text}]},
		})
	)
}

/// Ask the Harness to abandon the turn it is working on. This is the native
/// cancellation Craft 1.4 prefers over signal escalation, and it leaves the
/// Harness running for the input queued behind it.
pub(crate) fn interrupt_request(request: Uuid) -> String {
	format!(
		"{}\n",
		json!({
			"type": "control_request",
			"request_id": request.to_string(),
			"request": {"subtype": "interrupt"},
		})
	)
}
