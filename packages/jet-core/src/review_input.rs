//! The bounded visible transcript one reviewer is shown, and nothing else.
//!
//! A reviewer sees what a person looking at the Conversation would have
//! seen: the input the user submitted and the output the Harness produced,
//! oldest first and cut to a fixed budget from the newest end. It never
//! sees credentials, Settings, file contents, terminal output, or any part
//! of Jet's own state, and the exact requested action travels beside the
//! transcript rather than inside it.

use crate::{
	ApprovalRequest, Core, CoreError, Event, EventKind, ReviewInput, RunId,
	review::{TRANSCRIPT_BYTES, unavailable},
};

/// How the transcript labels who said what. The reviewer is told which
/// side each entry came from, because that is what "the user asked for
/// this" is judged against.
const USER: &str = "user";
const HARNESS: &str = "harness";

impl Core {
	/// Reads the visible transcript of `run_id`'s Conversation and pairs it
	/// with the exact requested action.
	///
	/// # Errors
	/// Returns a store error, or a conflict when the Run is gone.
	pub(crate) async fn review_input(
		&self,
		run_id: RunId,
		request: &ApprovalRequest,
	) -> Result<ReviewInput, CoreError> {
		let records = self
			.store
			.read(async |tx| {
				let run = tx
					.run(run_id.0)
					.await?
					.ok_or_else(|| unavailable("review.run_unavailable"))?;
				Ok::<_, CoreError>(
					tx.review_transcript_events(run.conversation_id).await?,
				)
			})
			.await?;
		let mut entries = Vec::new();
		for record in records {
			let event = Event::try_from(record)?;
			match event.kind {
				EventKind::TurnInput { text, .. } => {
					entries.push((USER, text));
				}
				EventKind::RunOutput {
					presentation_json, ..
				} if !presentation_json.is_empty() => {
					// Only the portable views a GUI renders are the visible
					// transcript. The native event beside them is lossless
					// Harness output — tool results, file content, terminal
					// bytes — and a reviewer is shown none of it.
					entries.push((HARNESS, presentation_json.join("\n")));
				}
				_ => continue,
			}
		}
		Ok(ReviewInput {
			transcript: transcript(&entries),
			tool: request.tool.clone(),
			action: request.action.clone(),
		})
	}
}

/// Renders the newest entries that fit the budget, oldest first. An entry
/// too long for what is left is cut rather than dropped: the newest entry
/// is the one the requested action follows from, and a reviewer judging
/// authorization against an empty transcript would judge nothing.
fn transcript(entries: &[(&str, String)]) -> String {
	let mut selected = Vec::new();
	let mut remaining = TRANSCRIPT_BYTES;
	for (role, content) in entries.iter().rev() {
		if remaining == 0 {
			break;
		}
		let line = format!("{role}: {}", one_line(content));
		let line = cut(&line, remaining.min(line.len()));
		remaining -= line.len();
		remaining = remaining.saturating_sub(1);
		selected.push(line);
	}
	selected.reverse();
	selected.join("\n")
}

/// Cuts `text` to at most `limit` bytes on a character boundary.
fn cut(text: &str, limit: usize) -> String {
	let mut end = text.len().min(limit);
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	text[..end].into()
}

/// Keeps one entry to a single line so no content can forge the structure
/// of the transcript around it.
fn one_line(content: &str) -> String {
	content
		.chars()
		.map(|c| if c.is_control() { ' ' } else { c })
		.collect()
}
