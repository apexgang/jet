//! Translates native Harness lines and approval input.

use super::{Turn, approval, harness, native, presentation};
use jet_craft_sdk::{CraftError, CraftSender};
use jet_protocol::{
	CraftEvent, Frame, FrameReader, FrameWriter, HelperCommand, RunActivity,
	TurnOutcome, decode_control, encode_control,
};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// One complete native line. The event is forwarded whole before anything is
/// concluded from it, and a line that is not JSON is left to the source.
pub(super) async fn line_observed(
	line: &[u8],
	turn: &mut Turn,
	sender: &mut CraftSender<OwnedWriteHalf>,
	writer: &mut FrameWriter<OwnedWriteHalf>,
	minor: u32,
) -> Result<(), CraftError> {
	let Ok(native_event) =
		serde_json::from_slice::<Box<serde_json::value::RawValue>>(line)
	else {
		return Ok(());
	};
	let Ok(value) =
		serde_json::from_str::<serde_json::Value>(native_event.get())
	else {
		return Ok(());
	};
	sender
		.send(&CraftEvent::Output {
			presentation: presentation::views(&value),
			native_event,
		})
		.await?;
	if minor >= 7 {
		for usage in crate::usage::reports(
			&value,
			&turn.id,
			std::time::SystemTime::now(),
		) {
			sender.send(&CraftEvent::Usage { usage }).await?;
		}
	}
	// A message for this Craft's own MCP server is answered here, except
	// the one that asks permission: the Harness waits for Jet on that.
	if let Some(observed) = turn
		.remote
		.as_mut()
		.and_then(|remote| remote.observe(&value))
	{
		return match observed {
			crate::execution::remote_tools::Observed::Reply(reply) => {
				input(writer, reply).await
			}
			crate::execution::remote_tools::Observed::Call(call) => {
				sender.send(&CraftEvent::RemoteTool { call }).await
			}
		};
	}
	match approval::served(&value) {
		approval::Served::Reply(reply) => return input(writer, reply).await,
		approval::Served::Asking(request) => {
			// The host is told what was asked before it is told the Harness
			// is waiting, so nothing has to guess which request the wait is
			// about (Craft 1.7).
			if minor >= 7 {
				sender
					.send(&CraftEvent::ApprovalRequested {
						request: request.asked(),
					})
					.await?;
			}
			turn.asking = Some(request);
			return sender
				.send(&CraftEvent::Activity {
					activity: RunActivity::WaitingForApproval,
				})
				.await;
		}
		approval::Served::Ignored => {}
	}
	match native::meaning(&value) {
		native::Meaning::Started { version } => {
			if minor >= 10
				&& let Some(model) =
					value.get("model").and_then(serde_json::Value::as_str)
			{
				sender
					.send(&CraftEvent::Model {
						model: model.into(),
					})
					.await?;
			}
			// ADR-0104: a release outside the matrix still runs, but never
			// silently. The warning is a diagnostic, not a Conversation
			// Event: the native init event already carries the release.
			if !version.starts_with(harness::TESTED_VERSIONS) {
				eprintln!(
					"jet-craft-claude: Claude Code {version} is outside the tested {} matrix",
					harness::TESTED_VERSIONS
				);
			}
			sender
				.send(&CraftEvent::Activity {
					activity: RunActivity::Working,
				})
				.await
		}
		native::Meaning::QuotaExhausted => {
			sender
				.send(&CraftEvent::Activity {
					activity: RunActivity::WaitingForQuota,
				})
				.await
		}
		native::Meaning::TurnResult {
			native_conversation,
		} => {
			sender
				.send(&CraftEvent::Completed {
					id: turn.id.clone(),
					native_conversation,
				})
				.await?;
			// A turn that arrived as its own Command reports its boundary;
			// the Run's initial input was captured before it started.
			if turn.queued && minor >= 3 {
				sender
					.send(&CraftEvent::TurnEnded {
						outcome: if turn.cancelling {
							TurnOutcome::Interrupted
						} else {
							TurnOutcome::Completed
						},
					})
					.await?;
			}
			turn.cancelling = false;
			Ok(())
		}
		native::Meaning::Opaque => Ok(()),
	}
}

pub(super) async fn input(
	writer: &mut FrameWriter<OwnedWriteHalf>,
	text: String,
) -> Result<(), CraftError> {
	ask(writer, &HelperCommand::Input { text }).await
}

pub(super) async fn ask(
	writer: &mut FrameWriter<OwnedWriteHalf>,
	message: &impl serde::Serialize,
) -> Result<(), CraftError> {
	let payload =
		encode_control(message).map_err(|_| CraftError::InvalidMessage)?;
	writer
		.write(&Frame::control(payload))
		.await
		.map_err(|_| CraftError::Disconnected)
}

pub(super) async fn hear<T: serde::de::DeserializeOwned>(
	reader: &mut FrameReader<OwnedReadHalf>,
) -> Result<T, CraftError> {
	match reader.read().await.map_err(|_| CraftError::Disconnected)? {
		Frame::Control { stream_id, payload } if stream_id.is_connection() => {
			decode_control(&payload).map_err(|_| CraftError::InvalidMessage)
		}
		Frame::Control { .. } | Frame::Data { .. } => {
			Err(CraftError::InvalidMessage)
		}
	}
}
