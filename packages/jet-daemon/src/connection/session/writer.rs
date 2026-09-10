//! Writes bounded replies and returns stream credit.

use super::MAX_PENDING_REPLIES;
use jet_protocol::{
	Frame, FrameWriter, OutboundQueue, ServerMessage, StreamControl,
	decode_control,
};
use tokio::{net::unix::OwnedWriteHalf, sync::mpsc};

pub(super) async fn write_replies(
	mut writer: FrameWriter<OwnedWriteHalf>,
	mut replies: mpsc::Receiver<Frame>,
) {
	let mut queue = OutboundQueue::new(0);
	while let Some(frame) = replies.recv().await {
		if queue_reply(&mut queue, frame).is_err() {
			return;
		}
		for _ in 1..MAX_PENDING_REPLIES {
			let Ok(frame) = replies.try_recv() else {
				break;
			};
			if queue_reply(&mut queue, frame).is_err() {
				return;
			}
		}
		loop {
			match queue.write_next(&mut writer).await {
				Ok(true) => {}
				Ok(false) => break,
				Err(_) => return,
			}
		}
	}
}

pub(super) fn queue_reply(
	queue: &mut OutboundQueue,
	frame: Frame,
) -> Result<(), jet_protocol::StreamQueueError> {
	let ordered = match &frame {
		Frame::Data { .. } => true,
		Frame::Control { payload, .. } => {
			matches!(
				decode_control::<ServerMessage>(payload),
				Ok(ServerMessage::Error { id: None, .. })
			) && !frame.stream_id().is_connection()
				|| matches!(
					decode_control::<StreamControl>(payload),
					Ok(StreamControl::TerminalGap { .. }
						| StreamControl::TerminalFinished { .. }
						| StreamControl::ArtifactFinished { .. })
				) || matches!(
				decode_control::<ServerMessage>(payload),
				Ok(ServerMessage::TerminalAttached { .. }
					| ServerMessage::TerminalResized { .. })
			) || decode_control::<jet_protocol::ArtifactControl>(payload)
				.is_ok()
		}
	};
	if ordered {
		queue.queue_terminal_frame(frame)
	} else {
		queue.queue_control(frame)
	}
}
