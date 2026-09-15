//! Codex app-server JSON-RPC messages pinned to the tested protocol release.
use serde_json::{Value, json};

/// Exact Codex release used for the v1 conformance matrix (ADR-0104).
pub(crate) const TESTED_VERSION: &str = "0.153.4";

/// Whether the app-server identified the exact release in this Craft's matrix.
pub(crate) fn tested(user_agent: &str) -> bool {
	user_agent
		.split_ascii_whitespace()
		.find_map(|part| part.strip_prefix("codex-cli/"))
		== Some(TESTED_VERSION)
}

pub(crate) fn arguments() -> Vec<String> {
	vec!["app-server".into(), "--stdio".into()]
}

pub(crate) fn initialize() -> String {
	line(json!({
		"id": 0,
		"method": "initialize",
		"params": {"clientInfo": {
			"name": "jet", "title": "Jet",
			"version": env!("CARGO_PKG_VERSION"),
		}},
	}))
}

pub(crate) fn start_thread() -> String {
	format!(
		"{}{}",
		line(json!({"method": "initialized"})),
		line(json!({"id": 1, "method": "thread/start", "params": {}})),
	)
}

pub(crate) fn start_turn(request: u64, thread: &str, text: &str) -> String {
	line(json!({
		"id": request,
		"method": "turn/start",
		"params": {"threadId": thread, "input": [{"type": "text", "text": text}]},
	}))
}

pub(crate) fn interrupt(request: u64, thread: &str, turn: &str) -> String {
	line(json!({
		"id": request,
		"method": "turn/interrupt",
		"params": {"threadId": thread, "turnId": turn},
	}))
}

fn line(value: Value) -> String {
	format!("{value}\n")
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	/// The published matrix pins the release this Craft's warning enforces
	/// (ADR-0104), so neither can move without the other.
	#[test]
	fn the_published_matrix_pins_this_release() {
		let matrix = include_str!("../../../../docs/conformance-matrix.md");
		let pinned = matrix
			.lines()
			.find(|line| line.starts_with("| Codex |"))
			.and_then(|line| line.split('|').nth(2))
			.map(str::trim);
		assert_eq!(
			pinned,
			Some(format!("Codex CLI {TESTED_VERSION}, exact")).as_deref()
		);
	}
}
