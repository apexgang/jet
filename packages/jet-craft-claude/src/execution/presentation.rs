//! Portable views that accompany, never replace, the native event (ADR-0008).
//!
//! A GUI renders these without Claude Code specific code, and a receiver that
//! understands the Harness still has every original byte to work from.
use jet_protocol::{Presentation, PresentationBlock};
use serde_json::Value;

/// Views stay small so the host's bounded observation budget is spent on the
/// native event rather than on a second copy of it. Text past this is dropped
/// from the view alone.
const VIEW_BYTES: usize = 4096;
/// One native event describes one model response, so a handful of blocks is
/// already generous; the rest of a very long response stays in the event.
const VIEW_BLOCKS: usize = 16;

/// Build the portable views for one native event. An event with nothing
/// portable to show yields none, which is not a failure.
pub(crate) fn views(event: &Value) -> Vec<PresentationBlock> {
	let Some("assistant") = event.get("type").and_then(Value::as_str) else {
		return vec![];
	};
	let Some(blocks) =
		event.pointer("/message/content").and_then(Value::as_array)
	else {
		return vec![];
	};
	blocks
		.iter()
		.filter_map(view)
		.filter_map(|view| PresentationBlock::new(&view).ok())
		.take(VIEW_BLOCKS)
		.collect()
}

/// One content block's portable view. A tool call is named rather than
/// described: its arguments are the native event's business, and rendering
/// them generically is how a view stops being inert.
fn view(block: &Value) -> Option<Presentation> {
	let text = |field: &str| {
		block
			.get(field)
			.and_then(Value::as_str)
			.filter(|text| !text.trim().is_empty())
			.map(bound)
	};
	match block.get("type").and_then(Value::as_str)? {
		"text" => Some(Presentation::Markdown {
			text: text("text")?,
		}),
		"thinking" => Some(Presentation::Text {
			text: text("thinking")?,
		}),
		"tool_use" => Some(Presentation::Text {
			text: text("name")?,
		}),
		_ => None,
	}
}

/// Truncate on a character boundary so a view is always valid UTF-8.
fn bound(text: &str) -> String {
	if text.len() <= VIEW_BYTES {
		return text.to_owned();
	}
	let mut end = VIEW_BYTES;
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	format!("{}…", &text[..end])
}
