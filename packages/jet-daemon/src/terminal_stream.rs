//! Per-connection terminal streams: bounded credit, independent cursors, raw bytes.
use jet_core::{Actor, Core, TerminalId, TerminalOperation};
use jet_protocol::{
	ClientMessage, Frame, ServerMessage, StreamControl, StreamId, WireError,
	encode_control,
};
use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::sync::mpsc;

const MAX_CREDIT: u64 = 16 * 1024 * 1024;
pub(crate) struct Terminals {
	data_limit: u32,
	streams: HashMap<StreamId, Attached>,
	tasks: tokio::task::JoinSet<StreamId>,
}
struct Attachment {
	stream: StreamId,
	id: TerminalId,
	after: u64,
	credit: u64,
}
struct Delivery<'a> {
	core: &'a Core,
	actor: &'a Actor,
	replies: &'a mpsc::Sender<Frame>,
	data_limit: u32,
}
struct Budget {
	remaining: Mutex<u64>,
	wake: tokio::sync::Notify,
}
struct Attached {
	id: TerminalId,
	credit: Arc<Budget>,
	start: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Terminals {
	pub(crate) fn new(data_limit: u32) -> Self {
		Self {
			data_limit,
			streams: HashMap::new(),
			tasks: tokio::task::JoinSet::new(),
		}
	}
	pub(crate) fn start(&mut self, stream: StreamId) {
		if let Some(attached) = self.streams.get_mut(&stream)
			&& let Some(start) = attached.start.take()
		{
			let _ = start.send(());
		}
	}
	pub(crate) fn contains(&self, stream: StreamId) -> bool {
		self.streams.contains_key(&stream)
	}
	pub(crate) async fn control(
		&mut self,
		stream: StreamId,
		message: &ClientMessage,
		core: &Arc<Core>,
		actor: &Actor,
		minor: u32,
		replies: &mpsc::Sender<Frame>,
	) -> Option<ServerMessage> {
		while let Some(Ok(stream)) = self.tasks.try_join_next() {
			self.streams.remove(&stream);
		}
		let (id, result) = match *message {
			ClientMessage::AttachTerminal {
				id,
				terminal_id,
				after,
				credit,
			} => {
				let result = if minor < jet_protocol::WORKSPACE_TERMINALS_MINOR
					|| self.contains(stream)
					|| self.streams.len() >= 16
					|| credit > MAX_CREDIT
				{
					Err(invalid())
				} else {
					self.attach(
						Attachment {
							stream,
							id: TerminalId(terminal_id),
							after,
							credit,
						},
						core,
						actor,
						minor,
						replies,
					)
					.await
				};
				(id, result.map(|()| ServerMessage::TerminalAttached { id }))
			}
			ClientMessage::ResizeTerminal { id, rows, columns } => {
				let result = match self.streams.get(&stream) {
					Some(attached) => core
						.terminal_operation(
							actor,
							attached.id,
							TerminalOperation::Resize { rows, columns },
						)
						.await
						.map(|_| ServerMessage::TerminalResized { id })
						.map_err(|e| crate::translate::error(e, minor)),
					None => Err(invalid()),
				};
				(id, result)
			}
			ClientMessage::Command { .. }
			| ClientMessage::Query { .. }
			| ClientMessage::RemoteTool { .. } => {
				return None;
			}
		};
		Some(result.unwrap_or_else(|error| ServerMessage::Error {
			id: Some(id),
			error,
		}))
	}
	async fn attach(
		&mut self,
		request: Attachment,
		core: &Arc<Core>,
		actor: &Actor,
		minor: u32,
		replies: &mpsc::Sender<Frame>,
	) -> Result<(), WireError> {
		let Attachment {
			stream,
			id,
			after,
			credit,
		} = request;
		core.terminal_operation(
			actor,
			id,
			TerminalOperation::Read { after, limit: 0 },
		)
		.await
		.map_err(|e| crate::translate::error(e, minor))?;
		let budget = Arc::new(Budget {
			remaining: Mutex::new(credit),
			wake: tokio::sync::Notify::new(),
		});
		let (start, started) = tokio::sync::oneshot::channel();
		self.streams.insert(
			stream,
			Attached {
				id,
				credit: Arc::clone(&budget),
				start: Some(start),
			},
		);
		let (core, actor, replies) =
			(Arc::clone(core), actor.clone(), replies.clone());
		// The first pump waits for its explicit start, sent after the attach reply.
		let data_limit = self.data_limit;
		self.tasks.spawn(async move {
			if started.await.is_err() {
				return stream;
			}
			if let Err(error) = pump(
				stream,
				id,
				after,
				budget,
				Delivery {
					core: &core,
					actor: &actor,
					replies: &replies,
					data_limit,
				},
			)
			.await
			{
				let reply = ServerMessage::Error {
					id: None,
					error: crate::translate::error(error, minor),
				};
				let _ = send(&replies, stream, &reply).await;
			}
			stream
		});
		Ok(())
	}
	pub(crate) async fn input(
		&self,
		stream: StreamId,
		bytes: Vec<u8>,
		core: &Core,
		actor: &Actor,
		minor: u32,
	) -> Result<(), WireError> {
		let attached = self.streams.get(&stream).ok_or_else(|| {
			crate::connection::wire_error(
				jet_protocol::ErrorCategory::InvalidInput,
				"protocol.unexpected_data_frame",
				"no inbound binary stream is open on this connection".into(),
			)
		})?;
		attached.credit.wake.notify_one();
		core.terminal_operation(
			actor,
			attached.id,
			TerminalOperation::Input(bytes),
		)
		.await
		.map(|_| ())
		.map_err(|e| crate::translate::error(e, minor))
	}
	pub(crate) fn credit(
		&self,
		stream: StreamId,
		bytes: u64,
	) -> Result<(), WireError> {
		let attached = self.streams.get(&stream).ok_or_else(|| {
			crate::connection::wire_error(
				jet_protocol::ErrorCategory::InvalidInput,
				"protocol.unknown_stream",
				"credit addressed a binary stream that is not open".into(),
			)
		})?;
		let mut credit =
			attached.credit.remaining.lock().expect("terminal credit");
		*credit = credit
			.checked_add(bytes)
			.filter(|sum| *sum <= MAX_CREDIT)
			.ok_or_else(invalid)?;
		attached.credit.wake.notify_one();
		Ok(())
	}
}
async fn pump(
	stream: StreamId,
	id: TerminalId,
	mut after: u64,
	budget: Arc<Budget>,
	delivery: Delivery<'_>,
) -> Result<(), jet_core::CoreError> {
	let Delivery {
		core,
		actor,
		replies,
		data_limit,
	} = delivery;
	let mut failed_reads = 0;
	let mut idle_ms = 25;
	loop {
		let limit = (*budget.remaining.lock().expect("terminal credit"))
			.min(u64::from(data_limit)) as u32;
		let output = match core
			.terminal_operation(
				actor,
				id,
				TerminalOperation::Read { after, limit },
			)
			.await
		{
			Ok(output) => {
				failed_reads = 0;
				output
			}
			Err(error)
				if error.category == jet_core::ErrorCategory::Unavailable
					&& failed_reads < 3 =>
			{
				failed_reads += 1;
				tokio::time::sleep(Duration::from_millis(25)).await;
				continue;
			}
			Err(error) => return Err(error),
		};
		if output.offset < after
			|| output.offset + output.bytes.len() as u64 > output.produced
		{
			return Err(stream_failed());
		}
		if output.offset > after {
			send(
				replies,
				stream,
				&StreamControl::TerminalGap {
					first_missing_offset: after,
					missing_bytes: output.offset - after,
				},
			)
			.await
			.map_err(|_| stream_failed())?;
		}
		after = output.offset + output.bytes.len() as u64;
		if !output.bytes.is_empty() {
			idle_ms = 25;
			*budget.remaining.lock().expect("terminal credit") -=
				output.bytes.len() as u64;
			replies
				.send(Frame::data(stream, output.bytes))
				.await
				.map_err(|_| stream_failed())?;
		}
		if output.closed && after == output.produced {
			send(
				replies,
				stream,
				&StreamControl::TerminalFinished { total_bytes: after },
			)
			.await
			.map_err(|_| stream_failed())?;
			return Ok(());
		}
		if after == output.produced || limit == 0 {
			tokio::select! {
				_ = tokio::time::sleep(Duration::from_millis(idle_ms)) => { idle_ms = (idle_ms * 2).min(250); },
				_ = budget.wake.notified() => { idle_ms = 25; },
			}
		}
	}
}
async fn send(
	replies: &mpsc::Sender<Frame>,
	stream: StreamId,
	value: &impl serde::Serialize,
) -> Result<(), ()> {
	let payload = encode_control(value).map_err(|_| ())?;
	replies
		.send(Frame::stream_control(stream, payload))
		.await
		.map_err(|_| ())
}
fn invalid() -> WireError {
	crate::connection::wire_error(
		jet_protocol::ErrorCategory::InvalidInput,
		"terminal.invalid_stream",
		"terminal stream, credit, or protocol version is invalid".into(),
	)
}
fn stream_failed() -> jet_core::CoreError {
	jet_core::CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: "terminal.stream_closed".into(),
		retryable: true,
		message: "reconnect the terminal stream from its last received byte"
			.into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
