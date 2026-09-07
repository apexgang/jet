//! The Claude Code native protocol, which is structured throughout (ADR-0002).
//!
//! `claude --print --input-format stream-json --output-format stream-json`
//! reads newline-delimited JSON for as long as its standard input is open and
//! writes the same on its output. Every later turn is a user message written
//! there, and the process ends when that input closes, so nothing in this
//! Craft inspects a terminal.
use serde_json::json;
use uuid::Uuid;

/// The Claude Code releases this Craft is tested against (ADR-0104). A Harness
/// outside this range still runs, and the matrix in `docs/claude-code-craft.md`
/// records what was verified.
pub(crate) const TESTED_VERSIONS: &str = "2.1";

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
		"--permission-prompts",
		"host",
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
