//! Reconnectable Workspace terminal streams beside ordinary request traffic.

use super::*;
use jet_protocol::StreamControl;

const MAX_TERMINAL_STREAMS: usize = 16;
const MAX_TERMINAL_CREDIT: u64 = 16 * 1024 * 1024;

pub(super) type Terminals = Arc<Mutex<HashMap<StreamId, TerminalReply>>>;

#[derive(Debug)]
pub(super) struct TerminalReply {
	reply: mpsc::Sender<Frame>,
	_permit: OwnedSemaphorePermit,
}

/// One attached terminal stream. Dropping it detaches the client but does not
/// close the Workspace-owned terminal.
pub struct TerminalAttachment<'a> {
	client: &'a Client,
	stream: StreamId,
	replies: mpsc::Receiver<Frame>,
	next_offset: u64,
	completed: bool,
}

/// Ordered output from one attached terminal stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
	/// Raw PTY output beginning at the supplied source offset.
	Output {
		/// Source offset of the first returned byte.
		offset: u64,
		/// Raw PTY bytes in source order.
		bytes: Vec<u8>,
	},
	/// A contiguous output range fell out of the terminal's rolling spool.
	Gap {
		/// First source offset no longer available.
		first_missing_offset: u64,
		/// Number of unavailable bytes.
		missing_bytes: u64,
	},
	/// The terminal stream ended at this source offset.
	Finished {
		/// Final produced source offset.
		total_bytes: u64,
	},
	/// The attached PTY accepted a size change.
	Resized,
}

impl Client {
	/// Attaches to an open Workspace terminal from an explicit output cursor.
	///
	/// # Errors
	/// Returns a compatibility, validation, authorization, protocol, or
	/// transport error.
	pub async fn attach_terminal(
		&self,
		terminal_id: Uuid,
		after: u64,
		credit: u64,
	) -> Result<TerminalAttachment<'_>, ClientError> {
		self.require_minor(jet_protocol::WORKSPACE_TERMINALS_MINOR)?;
		if credit == 0 || credit > MAX_TERMINAL_CREDIT {
			return Err(terminal_error(
				"terminal credit is outside the protocol bound",
			));
		}
		let permit = Arc::clone(&self.in_flight)
			.acquire_owned()
			.await
			.map_err(|_| ClientError::Closed)?;
		let stream = self.request_stream();
		let (reply, replies) = mpsc::channel(32);
		{
			let mut terminals =
				self.terminals.lock().expect("Terminal replies");
			if terminals.len() >= MAX_TERMINAL_STREAMS {
				return Err(terminal_error(
					"too many terminal streams are attached",
				));
			}
			terminals.insert(
				stream,
				TerminalReply {
					reply,
					_permit: permit,
				},
			);
		}
		let id = self.next_id();
		let mut attachment = TerminalAttachment {
			client: self,
			stream,
			replies,
			next_offset: after,
			completed: false,
		};
		if let Err(error) = self
			.send_on(
				stream,
				&ClientMessage::AttachTerminal {
					id,
					terminal_id,
					after,
					credit,
				},
			)
			.await
		{
			self.terminals
				.lock()
				.expect("Terminal replies")
				.remove(&stream);
			return Err(error);
		}
		let frame = attachment.receive_frame().await?;
		let Frame::Control { payload, .. } = frame else {
			return Err(terminal_error("terminal attached with a data frame"));
		};
		match decode_control::<ServerMessage>(&payload)? {
			ServerMessage::TerminalAttached { id: reply_id }
				if reply_id == id =>
			{
				Ok(attachment)
			}
			ServerMessage::Error {
				id: reply_id,
				error,
			} if reply_id.is_none() || reply_id == Some(id) => {
				attachment.completed = true;
				Err(ClientError::Remote(error))
			}
			_ => Err(terminal_error(
				"terminal attach reply did not match its request",
			)),
		}
	}
}

impl TerminalAttachment<'_> {
	/// Receives the next ordered terminal output, gap, resize acknowledgement,
	/// or finish marker.
	///
	/// # Errors
	/// Returns a remote, framing, contract, or transport error.
	pub async fn receive(&mut self) -> Result<TerminalEvent, ClientError> {
		let frame = self.receive_frame().await?;
		match frame {
			Frame::Data { payload, .. } if !payload.is_empty() => {
				let offset = self.next_offset;
				self.next_offset = self
					.next_offset
					.checked_add(payload.len() as u64)
					.ok_or_else(|| {
						terminal_error("terminal offset overflowed")
					})?;
				Ok(TerminalEvent::Output {
					offset,
					bytes: payload,
				})
			}
			Frame::Data { .. } => {
				Err(terminal_error("terminal sent an empty data frame"))
			}
			Frame::Control { payload, .. } => {
				if let Ok(ServerMessage::TerminalResized { .. }) =
					decode_control::<ServerMessage>(&payload)
				{
					return Ok(TerminalEvent::Resized);
				}
				if let Ok(ServerMessage::Error { error, .. }) =
					decode_control::<ServerMessage>(&payload)
				{
					self.completed = true;
					return Err(ClientError::Remote(error));
				}
				match decode_control::<StreamControl>(&payload)? {
					StreamControl::TerminalGap {
						first_missing_offset,
						missing_bytes,
					} if first_missing_offset == self.next_offset
						&& missing_bytes > 0 =>
					{
						self.next_offset = self
							.next_offset
							.checked_add(missing_bytes)
							.ok_or_else(|| {
								terminal_error("terminal gap overflowed")
							})?;
						Ok(TerminalEvent::Gap {
							first_missing_offset,
							missing_bytes,
						})
					}
					StreamControl::TerminalFinished { total_bytes }
						if total_bytes == self.next_offset =>
					{
						self.completed = true;
						Ok(TerminalEvent::Finished { total_bytes })
					}
					_ => Err(terminal_error(
						"terminal stream violated its cursor contract",
					)),
				}
			}
		}
	}

	/// Writes one bounded raw input chunk. A successful return confirms only
	/// local transport delivery; a disconnect can leave the terminal outcome
	/// unknown.
	pub async fn input(&self, bytes: Vec<u8>) -> Result<(), ClientError> {
		if bytes.is_empty() || bytes.len() > self.client.data_limit.min(65_536)
		{
			return Err(terminal_error(
				"terminal input is outside the chunk bound",
			));
		}
		self.client
			.send_frame(Frame::data(self.stream, bytes))
			.await
	}

	/// Grants additional bounded terminal output credit.
	pub async fn credit(&self, bytes: u64) -> Result<(), ClientError> {
		if bytes == 0 || bytes > MAX_TERMINAL_CREDIT {
			return Err(terminal_error(
				"terminal credit is outside the protocol bound",
			));
		}
		self.client
			.send_on(self.stream, &StreamControl::Credit { bytes })
			.await
	}

	/// Requests a PTY resize on this attached terminal stream.
	pub async fn resize(
		&self,
		rows: u16,
		columns: u16,
	) -> Result<(), ClientError> {
		if !(1..=1000).contains(&rows) || !(1..=1000).contains(&columns) {
			return Err(terminal_error(
				"terminal dimensions are outside the protocol bound",
			));
		}
		let id = self.client.next_id();
		self.client
			.send_on(
				self.stream,
				&ClientMessage::ResizeTerminal { id, rows, columns },
			)
			.await
	}

	async fn receive_frame(&mut self) -> Result<Frame, ClientError> {
		self.replies.recv().await.ok_or(ClientError::Closed)
	}
}

impl Drop for TerminalAttachment<'_> {
	fn drop(&mut self) {
		self.client
			.terminals
			.lock()
			.expect("Terminal replies")
			.remove(&self.stream);
	}
}

pub(super) fn route(terminals: &Terminals, frame: &Frame) -> Result<bool, ()> {
	let mut terminals = terminals.lock().expect("Terminal replies");
	let Some(terminal) = terminals.get(&frame.stream_id()) else {
		return Ok(false);
	};
	match terminal.reply.try_send(frame.clone()) {
		Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
		Err(mpsc::error::TrySendError::Full(_)) => return Err(()),
	}
	let completed = if let Frame::Control { payload, .. } = frame {
		matches!(
			decode_control::<StreamControl>(payload),
			Ok(StreamControl::TerminalFinished { .. })
		) || matches!(
			decode_control::<ServerMessage>(payload),
			Ok(ServerMessage::Error { .. })
		)
	} else {
		false
	};
	if completed {
		terminals.remove(&frame.stream_id());
	}
	Ok(true)
}

fn terminal_error(message: &str) -> ClientError {
	ClientError::Unexpected(message.into())
}
