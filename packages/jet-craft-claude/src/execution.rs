//! One Run: its Craft connection on one side, its helper and the Harness on
//! the other. Nothing here owns a process; the helper does (ADR-0060).
use crate::{approval, harness, native, presentation, specification};
use jet_craft_sdk::{CraftConnection, CraftError, CraftReceiver, CraftSender};
use jet_protocol::{
	CraftAction, CraftCommand, CraftEvent, CraftSpecification, Frame,
	FrameReader, FrameWriter, HelperCommand, HelperEvent, HelperHello,
	HelperRecord, NativeInputMode, NativeStream, ProtocolFamily, ProtocolOffer,
	ProtocolVersion, RunActivity, TurnOutcome, decode_control, encode_control,
};
use tokio::net::{
	UnixStream,
	unix::{OwnedReadHalf, OwnedWriteHalf},
};
use uuid::Uuid;

/// The helper contract this Craft needs: a Harness that keeps reading its
/// input for as long as the Conversation lasts (Helper 1.3).
const HELPER: ProtocolVersion = ProtocolVersion { major: 1, minor: 3 };

/// Serve one execution connection until the host releases it.
///
/// # Errors
/// Fails on an incompatible peer, an invalid Command, or a lost transport.
/// The caller closes the connection; nothing here is retried.
pub(crate) async fn execution(
	stream: UnixStream,
	declaration: CraftSpecification,
) -> Result<(), CraftError> {
	let program = specification::harness_program(&declaration)?;
	let (read, write) = stream.into_split();
	let connection = CraftConnection::accept(read, write, declaration).await?;
	let execution_id = connection.hello().execution_id;
	let resume = connection.hello().resume.clone();
	let minor = connection.negotiated().version.minor;
	let (receiver, mut sender) = connection.split();

	// A bounded reader task keeps a partial Craft frame alive across the
	// select! below, which would otherwise abandon one mid-decode.
	let (commands, mut requests) = tokio::sync::mpsc::channel(8);
	tokio::spawn(async move {
		let mut receiver: CraftReceiver<OwnedReadHalf> = receiver;
		while let Ok(command) = receiver.receive().await {
			if commands.send(command).await.is_err() {
				break;
			}
		}
	});

	let first = requests.recv().await.ok_or(CraftError::Disconnected)?;
	let (remote, first) = match first {
		CraftCommand::ConfigureRemoteTools { selection } => (
			Some(crate::remote_tools::RemoteTools::new(selection)),
			requests.recv().await.ok_or(CraftError::Disconnected)?,
		),
		other => (None, other),
	};
	let mut turn = Turn {
		remote,
		..Turn::default()
	};
	let (helper_socket, initial) = match first {
		CraftCommand::Start {
			id,
			text,
			helper_socket,
		} => {
			turn.id = id;
			(helper_socket, Initial::Launch(text))
		}
		CraftCommand::Recover {
			id,
			helper_socket,
			source_offset,
			checkpoint,
		} => {
			turn.id = id;
			let restored: Checkpoint =
				serde_json::from_str(&checkpoint).unwrap_or_default();
			turn.pending = restored.pending;
			turn.asking = restored.asking;
			(helper_socket, Initial::Recover(source_offset))
		}
		CraftCommand::Turn { .. }
		| CraftCommand::Interrupt { .. }
		| CraftCommand::Action { .. }
		| CraftCommand::Acknowledge { .. }
		| CraftCommand::ConfigureRemoteTools { .. }
		| CraftCommand::RemoteToolResult { .. }
		| CraftCommand::Shutdown => return Err(CraftError::InvalidMessage),
	};

	let helper = UnixStream::connect(&helper_socket)
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

	match initial {
		Initial::Launch(text) => {
			ask(
				&mut writer,
				&HelperCommand::Launch {
					program,
					arguments: harness::arguments(
						execution_id,
						resume
							.as_ref()
							.map(|resume| resume.native_conversation.as_str()),
					),
					input: format!(
						"{}{}",
						harness::register_server(),
						harness::user_message(&text)
					),
					// The Conversation outlives its first turn, so the Harness
					// must keep reading (Helper 1.3).
					input_mode: NativeInputMode::Streaming,
				},
			)
			.await?;
		}
		Initial::Recover(source_offset) => {
			ask(&mut writer, &HelperCommand::Recover { source_offset }).await?;
			sender
				.send(&CraftEvent::RunRecovered {
					helper_pid: ready.helper_pid,
					source_offset,
				})
				.await?;
		}
	}

	loop {
		// Pinning keeps this record's partial frame alive while Commands are
		// taken; only a whole record ever leaves the select.
		let incoming = hear::<HelperRecord>(&mut reader);
		tokio::pin!(incoming);
		let record = loop {
			tokio::select! {
				record = &mut incoming => break record?,
				command = requests.recv() => {
					if command.is_none() {
						return Err(CraftError::Disconnected);
					}
					if request(
						command.expect("checked above"),
						&mut turn,
						&mut sender,
						&mut writer,
						minor,
					)
					.await? == Flow::Release
					{
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
			&mut turn,
			&mut sender,
			&mut writer,
			ready.helper_pid,
			minor,
		)
		.await?;
		while turn
			.remote
			.as_ref()
			.is_some_and(crate::remote_tools::RemoteTools::pending)
		{
			let command =
				requests.recv().await.ok_or(CraftError::Disconnected)?;
			if request(command, &mut turn, &mut sender, &mut writer, minor)
				.await? == Flow::Release
			{
				return Ok(());
			}
		}
		// Every semantic Event for this source record has been sent, so the
		// host may now durably commit them together with the cursor.
		sender
			.send(&CraftEvent::Progress {
				source_offset: record.source_offset,
				checkpoint: serde_json::to_string(&Checkpoint {
					pending: turn.pending.clone(),
					asking: turn.asking.clone(),
				})
				.unwrap_or_default(),
			})
			.await?;
		let CraftCommand::Acknowledge { source_offset } =
			requests.recv().await.ok_or(CraftError::Disconnected)?
		else {
			return Err(CraftError::InvalidMessage);
		};
		ask(&mut writer, &HelperCommand::Acknowledge { source_offset }).await?;
		if ended {
			break;
		}
	}
	Ok(())
}

/// What this connection does after one Command.
#[derive(PartialEq, Eq)]
enum Flow {
	/// Keep serving this execution.
	Continue,
	/// The host released this execution connection.
	Release,
}

/// How this connection begins: a fresh Harness, or a retained one.
enum Initial {
	Launch(String),
	Recover(u64),
}

/// The adapter state a Run's checkpoint carries, so a restarted Craft reads
/// the next source record exactly where the previous one stopped, and can
/// still answer a permission request the Harness is waiting on.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
	/// Native bytes before the next complete line.
	pending: Vec<u8>,
	/// A permission request the Harness has not been answered about.
	asking: Option<approval::Request>,
}

/// The turn currently in flight and the parser state behind it.
#[derive(Default)]
struct Turn {
	remote: Option<crate::remote_tools::RemoteTools>,
	/// Correlation identity the next native completion answers.
	id: String,
	/// Whether this turn arrived as its own Command rather than with the Run.
	queued: bool,
	/// Whether cancellation was asked for and is still unanswered.
	cancelling: bool,
	/// Native bytes before the next complete line, committed as a checkpoint.
	pending: Vec<u8>,
	/// A permission request the Harness is waiting on, if there is one. It
	/// is held rather than answered: this Craft never decides it.
	asking: Option<approval::Request>,
}

/// Carry out one admitted Command.
async fn request(
	command: CraftCommand,
	turn: &mut Turn,
	sender: &mut CraftSender<OwnedWriteHalf>,
	writer: &mut FrameWriter<OwnedWriteHalf>,
	minor: u32,
) -> Result<Flow, CraftError> {
	match command {
		CraftCommand::Turn { id, text } => {
			if minor >= 3 {
				sender.send(&CraftEvent::TurnStarted).await?;
			}
			*turn = Turn {
				id,
				queued: true,
				cancelling: false,
				pending: std::mem::take(&mut turn.pending),
				asking: turn.asking.take(),
				remote: turn.remote.take(),
			};
			input(writer, harness::user_message(&text)).await?;
			Ok(Flow::Continue)
		}
		// Native cancellation, which Craft 1.4 prefers: the Harness abandons
		// this turn and stays available for the input queued behind it.
		CraftCommand::Interrupt { .. } => {
			turn.cancelling = true;
			input(writer, harness::interrupt_request(Uuid::new_v4())).await?;
			Ok(Flow::Continue)
		}
		// Releasing the connection closes the Harness's input, which is how
		// this Harness ends: no signal, and its output stays retained.
		CraftCommand::ConfigureRemoteTools { .. } => {
			Err(CraftError::InvalidMessage)
		}
		CraftCommand::RemoteToolResult {
			operation_id,
			outcome,
		} => {
			let reply = turn
				.remote
				.as_mut()
				.ok_or(CraftError::InvalidMessage)?
				.reply(operation_id, outcome)?;
			input(writer, reply).await?;
			Ok(Flow::Continue)
		}
		CraftCommand::Shutdown => {
			ask(writer, &HelperCommand::CloseInput).await?;
			Ok(Flow::Release)
		}
		// Jet authorized this exact decision before it crossed the boundary,
		// and the Harness has been waiting for it since it asked.
		CraftCommand::Action {
			action: CraftAction::Approval {
				request_id,
				decision,
			},
			..
		} => {
			let asking = turn
				.asking
				.take_if(|asking| asking.id == request_id)
				.ok_or(CraftError::InvalidMessage)?;
			input(writer, asking.decided(decision)).await?;
			sender
				.send(&CraftEvent::Activity {
					activity: RunActivity::Working,
				})
				.await?;
			Ok(Flow::Continue)
		}
		// This Craft presents no native actions, so there is none to invoke.
		CraftCommand::Action { .. }
		| CraftCommand::Start { .. }
		| CraftCommand::Recover { .. }
		| CraftCommand::Acknowledge { .. } => Err(CraftError::InvalidMessage),
	}
}

/// Translate one native observation into the Events it stands for.
async fn observe(
	event: HelperEvent,
	turn: &mut Turn,
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
		// Diagnostics belong on standard error and describe nothing about
		// the Conversation, so they stay in the retained source alone.
		HelperEvent::Output {
			stream: NativeStream::Stderr,
			..
		} => Ok(()),
		HelperEvent::Output {
			stream: NativeStream::Stdout,
			bytes,
		} => {
			turn.pending.extend(bytes);
			while let Some(end) =
				turn.pending.iter().position(|byte| *byte == b'\n')
			{
				let line: Vec<u8> = turn.pending.drain(..=end).collect();
				line_observed(&line, turn, sender, writer, minor).await?;
			}
			Ok(())
		}
		HelperEvent::Exited { exit_code } => {
			sender.send(&CraftEvent::RunEnded { exit_code }).await
		}
	}
}

/// One complete native line. The event is forwarded whole before anything is
/// concluded from it, and a line that is not JSON is left to the source.
async fn line_observed(
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
	if minor >= 8 {
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
			crate::remote_tools::Observed::Reply(reply) => {
				input(writer, reply).await
			}
			crate::remote_tools::Observed::Call(call) => {
				sender.send(&CraftEvent::RemoteTool { call }).await
			}
		};
	}
	match approval::served(&value) {
		approval::Served::Reply(reply) => return input(writer, reply).await,
		approval::Served::Asking(request) => {
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
