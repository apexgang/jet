//! Portable views accompanying, never replacing, native Codex events.
use jet_protocol::{Presentation, PresentationBlock};
use serde_json::Value;

const VIEW_BYTES: usize = 4096;

pub(crate) fn views(event: &Value) -> Vec<PresentationBlock> {
	view(event)
		.and_then(|view| PresentationBlock::new(&view).ok())
		.into_iter()
		.collect()
}

fn view(event: &Value) -> Option<Presentation> {
	match event.get("method").and_then(Value::as_str)? {
		"turn/plan/updated" => plan(event),
		"item/completed" => item(event),
		_ => None,
	}
}

fn plan(event: &Value) -> Option<Presentation> {
	let steps = event.pointer("/params/plan")?.as_array()?;
	let text = steps
		.iter()
		.filter_map(|entry| {
			let step = entry.get("step")?.as_str()?.trim();
			let mark = match entry.get("status")?.as_str()? {
				"pending" => " ",
				"inProgress" => "~",
				"completed" => "x",
				_ => return None,
			};
			(!step.is_empty()).then(|| format!("- [{mark}] {step}"))
		})
		.collect::<Vec<_>>()
		.join("\n");
	(!text.is_empty()).then(|| Presentation::Markdown { text: bound(&text) })
}

fn item(event: &Value) -> Option<Presentation> {
	let item = event.pointer("/params/item")?;
	match item.get("type")?.as_str()? {
		"agentMessage" | "plan" => Some(Presentation::Markdown {
			text: bound(item.get("text")?.as_str()?.trim()),
		}),
		"reasoning" => {
			let text = item
				.get("summary")?
				.as_array()?
				.iter()
				.filter_map(Value::as_str)
				.collect::<Vec<_>>()
				.join("\n");
			(!text.trim().is_empty())
				.then(|| Presentation::Text { text: bound(&text) })
		}
		_ => None,
	}
}

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
