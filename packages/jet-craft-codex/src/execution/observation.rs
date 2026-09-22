//! Native observations and helper framing for one retained Codex execution.
use super::{Run, approval, harness, presentation, remote_tools};
use jet_craft_sdk::{CraftError, CraftSender};
use jet_protocol::{
	CraftEvent, Frame, FrameReader, FrameWriter, HelperCommand, HelperEvent,
	NativeStream, RunActivity, decode_control, encode_control,
};
use serde_json::{Value, value::RawValue};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

const NATIVE_LINE_BYTES: usize = 1024 * 1024;

pub(super) async fn observe(
	event: HelperEvent,
	run: &mut Run,
	sender: &mut CraftSender<OwnedWriteHalf>,
	writer: &mut FrameWriter<OwnedWriteHalf>,
	helper_pid: u32,
	minor: u32,
) -> Result<(), CraftError> {
	match event {
		HelperEvent::LaunchFailed => {
			sender.send(&CraftEvent::RunLaunchFailed).await
		}
		HelperEvent::Started { harness_pid } => {
			sender
				.send(&CraftEvent::RunStarted {
					helper_pid,
					harness_pid,
				})
				.await
		}
		HelperEvent::Output {
			stream: NativeStream::Stderr,
			..
		} => Ok(()),
		HelperEvent::Output {
			stream: NativeStream::Stdout,
			bytes,
		} => {
			// ASVS 2.2.1: a peer cannot grow partial JSON without a bound.
			if bytes.len() > NATIVE_LINE_BYTES.saturating_sub(run.pending.len())
			{
				return Err(CraftError::InvalidMessage);
			}
			run.pending.extend(bytes);
			while let Some(end) =
				run.pending.iter().position(|byte| *byte == b'\n')
			{
				let line: Vec<u8> = run.pending.drain(..=end).collect();
				line_observed(&line, run, sender, writer, minor).await?;
			}
			Ok(())
		}
		HelperEvent::Exited { exit_code } => {
			sender.send(&CraftEvent::RunEnded { exit_code }).await
		}
	}
}

async fn line_observed(
	line: &[u8],
	run: &mut Run,
	sender: &mut CraftSender<OwnedWriteHalf>,
	writer: &mut FrameWriter<OwnedWriteHalf>,
	minor: u32,
) -> Result<(), CraftError> {
	// ASVS 1.5.2/2.2.1: only bounded JSON lines become semantic events.
	let Ok(native_event) = serde_json::from_slice::<Box<RawValue>>(line) else {
		return Ok(());
	};
	let Ok(value) = serde_json::from_str::<Value>(native_event.get()) else {
		return Ok(());
	};
	sender
		.send(&CraftEvent::Output {
			native_event,
			presentation: presentation::views(&value),
		})
		.await?;
	if let Some(request) = approval::request(&value, run.thread.as_deref()) {
		if run.asking.is_some() {
			return Err(CraftError::InvalidMessage);
		}
		// What was asked reaches the host before the wait does, so nothing
		// has to guess which request the Harness is waiting on (Craft 1.7).
		if minor >= 7 {
			sender
				.send(&CraftEvent::ApprovalRequested {
					request: request.asked(),
				})
				.await?;
		}
		run.asking = Some(request);
		return sender
			.send(&CraftEvent::Activity {
				activity: RunActivity::WaitingForApproval,
			})
			.await;
	}
	if value.get("method").is_none() {
		match value.get("id").and_then(Value::as_u64) {
			Some(0) => {
				if let Some(version) =
					value.pointer("/result/userAgent").and_then(Value::as_str)
					&& !harness::tested(version)
				{
					eprintln!(
						"jet-craft-codex: Codex {version} is outside the tested {} matrix",
						harness::TESTED_VERSION,
					);
				}
				return run
					.input(
						writer,
						harness::start_thread(
							run.resume.as_ref(),
							run.remote
								.as_ref()
								.map(remote_tools::RemoteTools::configuration)
								.transpose()?,
						),
					)
					.await;
			}
			Some(1) => {
				let thread = value
					.pointer("/result/thread/id")
					.and_then(Value::as_str)
					.filter(|thread| !thread.is_empty())
					.ok_or(CraftError::InvalidMessage)?
					.to_owned();
				if run
					.resume
					.as_ref()
					.is_some_and(|resume| resume.native_conversation != thread)
				{
					return Err(CraftError::InvalidMessage);
				}
				let model = value
					.pointer("/result/model")
					.and_then(Value::as_str)
					.filter(|model| !model.is_empty() && model.len() <= 256);
				if run
					.resume
					.as_ref()
					.and_then(|resume| resume.model.as_deref())
					.is_some_and(|expected| Some(expected) != model)
				{
					return Err(CraftError::InvalidMessage);
				}
				run.model = model.map(str::to_owned);
				if minor >= 10
					&& let Some(model) = &run.model
				{
					sender
						.send(&CraftEvent::Model {
							model: model.clone(),
						})
						.await?;
				}
				run.thread = Some(thread.clone());
				let text = run
					.initial_text
					.take()
					.ok_or(CraftError::InvalidMessage)?;
				return run
					.input(
						writer,
						harness::start_turn(
							2,
							&thread,
							&text,
							run.model.as_deref(),
						),
					)
					.await;
			}
			Some(_) => {
				if let Some(turn) = value
					.pointer("/result/turn/id")
					.and_then(Value::as_str)
					.filter(|turn| !turn.is_empty())
				{
					run.native_turn = Some(turn.to_owned());
				}
			}
			None => {}
		}
	}
	match value.get("method").and_then(Value::as_str) {
		Some("turn/started") => {
			sender
				.send(&CraftEvent::Activity {
					activity: RunActivity::Working,
				})
				.await
		}
		Some("error") => {
			let activity = match value
				.pointer("/params/error/codexErrorInfo")
				.and_then(Value::as_str)
			{
				Some(
					"rateLimitExceeded"
					| "usageLimitExceeded"
					| "sessionBudgetExceeded",
				) => Some(RunActivity::WaitingForQuota),
				Some("unauthorized") => Some(RunActivity::WaitingForAuth),
				Some(_) | None => None,
			};
			if let Some(activity) = activity {
				sender.send(&CraftEvent::Activity { activity }).await?;
			}
			Ok(())
		}
		Some("thread/tokenUsage/updated") => {
			if minor >= 7 {
				let finality = if run.in_flight {
					jet_protocol::CraftUsageFinality::Interim
				} else {
					jet_protocol::CraftUsageFinality::Final
				};
				for usage in crate::usage::reports(&value, &run.id, finality) {
					sender.send(&CraftEvent::Usage { usage }).await?;
				}
			}
			Ok(())
		}
		Some("turn/completed") => {
			let thread = value
				.pointer("/params/threadId")
				.and_then(Value::as_str)
				.filter(|thread| Some(*thread) == run.thread.as_deref())
				.ok_or(CraftError::InvalidMessage)?;
			sender
				.send(&CraftEvent::Completed {
					id: run.id.clone(),
					native_conversation: thread.to_owned(),
				})
				.await?;
			if run.queued && minor >= 3 {
				let interrupted = value
					.pointer("/params/turn/status")
					.and_then(Value::as_str)
					== Some("interrupted")
					|| run.cancelling;
				sender
					.send(&CraftEvent::TurnEnded {
						outcome: if interrupted {
							jet_protocol::TurnOutcome::Interrupted
						} else {
							jet_protocol::TurnOutcome::Completed
						},
					})
					.await?;
			}
			run.in_flight = false;
			run.native_turn = None;
			run.cancelling = false;
			Ok(())
		}
		Some(_) | None => Ok(()),
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
