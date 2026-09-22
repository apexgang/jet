//! One Run between the Craft connection and a helper-owned app-server.
pub(crate) mod approval;
pub(crate) mod harness;
pub(crate) mod presentation;
pub(crate) mod remote_tools;

mod delivery;
mod observation;
use observation::{ask, hear, input, observe};

use crate::specification;
use jet_craft_sdk::{CraftConnection, CraftError, CraftReceiver, CraftSender};
use jet_protocol::{
	CraftAction, CraftCommand, CraftEvent, CraftSpecification, FrameReader,
	FrameWriter, HelperCommand, HelperEvent, HelperHello, HelperRecord,
	NativeInputMode, ProtocolFamily, ProtocolOffer, ProtocolVersion,
	RunActivity,
};
use tokio::net::{
	UnixStream,
	unix::{OwnedReadHalf, OwnedWriteHalf},
};

const HELPER: ProtocolVersion = ProtocolVersion { major: 1, minor: 3 };

/// Serve one execution connection until its native process ends.
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
	let (selection, first) = match first {
		CraftCommand::ConfigureRemoteTools { selection } => (
			Some(selection),
			requests.recv().await.ok_or(CraftError::Disconnected)?,
		),
		other => (None, other),
	};
	let (mut run, helper_socket, initial) = match first {
		CraftCommand::Start {
			id,
			text,
			helper_socket,
		} => (
			Run {
				id,
				resume,
				initial_text: Some(text),
				in_flight: true,
				next_request: 3,
				..Run::default()
			},
			helper_socket,
			Initial::Launch,
		),
		CraftCommand::Recover {
			helper_socket,
			source_offset,
			checkpoint,
			..
		} => (
			serde_json::from_str(&checkpoint)
				.map_err(|_| CraftError::InvalidMessage)?,
			helper_socket,
			Initial::Recover(source_offset),
		),
		_ => return Err(CraftError::InvalidMessage),
	};
	let committed = match initial {
		Initial::Launch => 0,
		Initial::Recover(offset) => offset,
	};
	run.delivery = Some(
		delivery::Delivery::open(
			std::path::Path::new(&helper_socket),
			committed,
		)
		.await?,
	);
	if let Some(selection) = selection {
		run.remote = Some(
			remote_tools::RemoteTools::bind(
				std::path::Path::new(&helper_socket),
				selection,
			)
			.await?,
		);
	}
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
	match initial {
		Initial::Launch => {
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
		let incoming = hear::<HelperRecord>(&mut reader);
		tokio::pin!(incoming);
		let record = loop {
			tokio::select! {
				record = &mut incoming => break record?,
				call = async {
					match &mut run.remote {
						Some(remote) => remote.next().await,
						None => std::future::pending().await,
					}
				} => { sender.send(&CraftEvent::RemoteTool { call: call? }).await?; },
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
		// The host answers remote calls before reading the next semantic batch.
		// Consume that reply before expecting this record's acknowledgement.
		while run
			.remote
			.as_ref()
			.is_some_and(remote_tools::RemoteTools::pending)
		{
			let command =
				requests.recv().await.ok_or(CraftError::Disconnected)?;
			if request(command, &mut run, &mut sender, &mut writer, minor)
				.await?
			{
				return Ok(());
			}
		}
		sender
			.send(&CraftEvent::Progress {
				source_offset: record.source_offset,
				checkpoint: serde_json::to_string(&run)
					.map_err(|_| CraftError::InvalidMessage)?,
			})
			.await?;
		let Some(CraftCommand::Acknowledge { source_offset }) =
			requests.recv().await
		else {
			return Err(CraftError::InvalidMessage);
		};
		if source_offset != record.source_offset {
			return Err(CraftError::InvalidMessage);
		}
		run.delivery
			.as_mut()
			.ok_or(CraftError::InvalidMessage)?
			.acknowledged(source_offset)
			.await?;
		ask(&mut writer, &HelperCommand::Acknowledge { source_offset }).await?;
		if ended {
			return Ok(());
		}
	}
}

enum Initial {
	Launch,
	Recover(u64),
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Run {
	#[serde(skip)]
	delivery: Option<delivery::Delivery>,
	#[serde(skip)]
	remote: Option<remote_tools::RemoteTools>,
	resume: Option<jet_protocol::CraftResume>,
	model: Option<String>,
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

impl Run {
	async fn input(
		&mut self,
		writer: &mut FrameWriter<OwnedWriteHalf>,
		text: String,
	) -> Result<(), CraftError> {
		self.delivery
			.as_mut()
			.ok_or(CraftError::InvalidMessage)?
			.before_input()
			.await?;
		input(writer, text).await
	}
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
			let native = harness::start_turn(
				run.next_request,
				&thread,
				&text,
				run.model.as_deref(),
			);
			run.next_request += 1;
			run.input(writer, native).await?;
			Ok(false)
		}
		CraftCommand::ConstrainSubagents { .. }
		| CraftCommand::ConfigureRemoteTools { .. } => {
			Err(CraftError::InvalidMessage)
		}
		CraftCommand::RemoteToolResult {
			operation_id,
			outcome,
		} => {
			run.remote
				.as_mut()
				.ok_or(CraftError::InvalidMessage)?
				.reply(operation_id, outcome)?;
			Ok(false)
		}
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
			run.input(writer, native).await?;
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
			run.input(writer, asking.decided(decision)).await?;
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
