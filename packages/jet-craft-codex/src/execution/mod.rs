//! One Run between the Craft connection and a helper-owned app-server.
pub(crate) mod approval;
pub(crate) mod harness;
pub(crate) mod presentation;

use crate::specification;
use jet_craft_sdk::{CraftConnection, CraftError, CraftReceiver, CraftSender};
use jet_protocol::{
	CraftAction, CraftCommand, CraftEvent, CraftSpecification, Frame,
	FrameReader, FrameWriter, HelperCommand, HelperEvent, HelperHello,
	HelperRecord, NativeInputMode, NativeStream, ProtocolFamily, ProtocolOffer,
	ProtocolVersion, RunActivity, decode_control, encode_control,
};
use serde_json::{Value, value::RawValue};
use tokio::net::{
	UnixStream,
	unix::{OwnedReadHalf, OwnedWriteHalf},
};

const HELPER: ProtocolVersion = ProtocolVersion { major: 1, minor: 3 };
const NATIVE_LINE_BYTES: usize = 1024 * 1024;

/// Serve one execution connection until its native process ends.
pub(crate) async fn execution(
	stream: UnixStream,
	declaration: CraftSpecification,
) -> Result<(), CraftError> {
	let program = specification::harness_program(&declaration)?;
	let (read, write) = stream.into_split();
	let connection = CraftConnection::accept(read, write, declaration).await?;
	let execution_id = connection.hello().execution_id;
	let minor = connection.negotiated().version.minor;
	let (receiver, mut sender) = connection.split();
	let (commands, mut requests) = tokio::sync::mpsc::channel(8);
	tokio::spawn(async move {
		let mut receiver: CraftReceiver<OwnedReadHalf> = receiver;
		while let Ok(command) = receiver.receive().await {
			if commands.send(command).await.is_err() {
				break;
			}
		}
	});

	let (id, text, helper_socket) = match requests.recv().await {
		Some(CraftCommand::Start {
			id,
			text,
			helper_socket,
		}) => (id, text, helper_socket),
		Some(_) => return Err(CraftError::InvalidMessage),
		None => return Err(CraftError::Disconnected),
	};
	let helper = UnixStream::connect(helper_socket)
		.await
		.map_err(|_| CraftError::Disconnected)?;
	let (read, write) = helper.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	ask(
		&mut writer,
		&HelperHello {
			execution_id,
			protocol: ProtocolOffer {
				family: ProtocolFamily::Helper,
				versions: vec![HELPER],
				capabilities: vec![],
			},
		},
	)
	.await?;
	let ready: jet_protocol::HelperReady = hear(&mut reader).await?;
	ask(
		&mut writer,
		&HelperCommand::Launch {
			program,
			arguments: harness::arguments(),
			input: harness::initialize(),
			input_mode: NativeInputMode::Streaming,
		},
	)
	.await?;
	let mut run = Run {
		id,
		initial_text: Some(text),
		in_flight: true,
		next_request: 3,
		..Run::default()
	};

	loop {
		let incoming = hear::<HelperRecord>(&mut reader);
		tokio::pin!(incoming);
		let record = loop {
			tokio::select! {
				record = &mut incoming => break record?,
				command = requests.recv() => {
					let Some(command) = command else {
						return Err(CraftError::Disconnected);
					};
					if request(command, &mut run, &mut sender, &mut writer, minor).await? {
						return Ok(());
					}
				}
			}
		};
		let ended = matches!(
			record.event,
			HelperEvent::Exited { .. } | HelperEvent::LaunchFailed
		);
		observe(
			record.event,
			&mut run,
			&mut sender,
			&mut writer,
			ready.helper_pid,
			minor,
		)
		.await?;
		sender
			.send(&CraftEvent::Progress {
				source_offset: record.source_offset,
				checkpoint: serde_json::to_string(&Checkpoint {
					pending: &run.pending,
					asking: &run.asking,
				})
				.unwrap_or_default(),
			})
			.await?;
		let Some(CraftCommand::Acknowledge { source_offset }) =
			requests.recv().await
		else {
			return Err(CraftError::InvalidMessage);
		};
		ask(&mut writer, &HelperCommand::Acknowledge { source_offset }).await?;
		if ended {
			return Ok(());
		}
	}
}

#[derive(Default)]
struct Run {
	id: String,
	initial_text: Option<String>,
	thread: Option<String>,
	pending: Vec<u8>,
	queued: bool,
	in_flight: bool,
	next_request: u64,
	asking: Option<approval::Request>,
	native_turn: Option<String>,
	cancelling: bool,
}

#[derive(serde::Serialize)]
struct Checkpoint<'a> {
	pending: &'a [u8],
	asking: &'a Option<approval::Request>,
}

async fn request(
	command: CraftCommand,
	run: &mut Run,
	sender: &mut CraftSender<OwnedWriteHalf>,
	writer: &mut FrameWriter<OwnedWriteHalf>,
	minor: u32,
) -> Result<bool, CraftError> {
	match command {
		CraftCommand::Turn { id, text } if !run.in_flight => {
			let thread =
				run.thread.clone().ok_or(CraftError::InvalidMessage)?;
			if minor >= 3 {
				sender.send(&CraftEvent::TurnStarted).await?;
			}
			run.id = id;
			run.queued = true;
			run.in_flight = true;
			let native = harness::start_turn(run.next_request, &thread, &text);
			run.next_request += 1;
			input(writer, native).await?;
			Ok(false)
		}
		CraftCommand::ConstrainSubagents { .. }
		| CraftCommand::ConfigureRemoteTools { .. }
		| CraftCommand::RemoteToolResult { .. } => Err(CraftError::InvalidMessage),
		CraftCommand::Shutdown => {
			ask(writer, &HelperCommand::CloseInput).await?;
			Ok(true)
		}
		CraftCommand::Interrupt { id }
			if run.in_flight && id == run.id && !run.cancelling =>
		{
			let thread =
				run.thread.clone().ok_or(CraftError::InvalidMessage)?;
			let turn =
				run.native_turn.clone().ok_or(CraftError::InvalidMessage)?;
			let native = harness::interrupt(run.next_request, &thread, &turn);
			run.next_request += 1;
			run.cancelling = true;
			input(writer, native).await?;
			Ok(false)
		}
		// ASVS 8.3.1: only the exact request authorized by Jet is answered.
		CraftCommand::Action {
			action: CraftAction::Approval {
				request_id,
				decision,
			},
			..
		} => {
			let asking = run
				.asking
				.take_if(|asking| asking.id == request_id)
				.ok_or(CraftError::InvalidMessage)?;
			input(writer, asking.decided(decision)).await?;
			sender
				.send(&CraftEvent::Activity {
					activity: RunActivity::Working,
				})
				.await?;
			Ok(false)
		}
		CraftCommand::Turn { .. }
		| CraftCommand::Start { .. }
		| CraftCommand::Recover { .. }
		| CraftCommand::Interrupt { .. }
		| CraftCommand::Action {
			action: CraftAction::Invoke { .. },
			..
		}
		| CraftCommand::Acknowledge { .. } => Err(CraftError::InvalidMessage),
	}
}

async fn observe(
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
				return input(writer, harness::start_thread()).await;
			}
			Some(1) => {
				let thread = value
					.pointer("/result/thread/id")
					.and_then(Value::as_str)
					.filter(|thread| !thread.is_empty())
					.ok_or(CraftError::InvalidMessage)?
					.to_owned();
				run.thread = Some(thread.clone());
				let text = run
					.initial_text
					.take()
					.ok_or(CraftError::InvalidMessage)?;
				return input(writer, harness::start_turn(2, &thread, &text))
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

async fn input(
	writer: &mut FrameWriter<OwnedWriteHalf>,
	text: String,
) -> Result<(), CraftError> {
	ask(writer, &HelperCommand::Input { text }).await
}

async fn ask(
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

async fn hear<T: serde::de::DeserializeOwned>(
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
