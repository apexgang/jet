//! Writes bounded replies and returns stream credit.

use super::MAX_PENDING_REPLIES;
use jet_protocol::{
	Frame, FrameWriter, OutboundQueue, RequestId, StreamControl, WireError,
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
			let reply = decode_control::<OrderedReply>(payload);
			matches!(reply, Ok(OrderedReply::Error { id: None, .. }))
				&& !frame.stream_id().is_connection()
				|| matches!(
					decode_control::<StreamControl>(payload),
					Ok(StreamControl::TerminalGap { .. }
						| StreamControl::TerminalFinished { .. }
						| StreamControl::ArtifactFinished { .. })
				) || matches!(
				reply,
				Ok(OrderedReply::TerminalAttached { .. }
					| OrderedReply::TerminalResized { .. })
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

/// The [`jet_protocol::ServerMessage`] kinds that decide how a reply is
/// queued, decoded exactly as that message decodes them. Every other kind
/// fails to decode, so classifying a reply compiles no decoder for the rest.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum OrderedReply {
	TerminalAttached {
		#[expect(
			dead_code,
			reason = "decoded so a reply matches only as its message does"
		)]
		id: RequestId,
	},
	TerminalResized {
		#[expect(
			dead_code,
			reason = "decoded so a reply matches only as its message does"
		)]
		id: RequestId,
	},
	Error {
		id: Option<RequestId>,
		// Boxed only to keep the variants alike in size; a box decodes
		// exactly as its contents do.
		#[expect(
			dead_code,
			reason = "decoded so a reply matches only as its message does"
		)]
		error: Box<WireError>,
	},
}

#[cfg(test)]
mod tests {
	use super::OrderedReply;
	use jet_protocol::{
		ErrorCategory, PlaneStatus, QueryResponse, RemoteToolResult,
		ServerMessage, WireError, decode_control, encode_control,
	};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	/// Whether a reply ends its connection, and whether it is a terminal
	/// acknowledgement.
	type Class = (bool, bool);

	fn class(reply: Option<OrderedReply>) -> Class {
		(
			matches!(reply, Some(OrderedReply::Error { id: None, .. })),
			matches!(
				reply,
				Some(
					OrderedReply::TerminalAttached { .. }
						| OrderedReply::TerminalResized { .. }
				)
			),
		)
	}

	fn full_class(message: Option<ServerMessage>) -> Class {
		(
			matches!(message, Some(ServerMessage::Error { id: None, .. })),
			matches!(
				message,
				Some(
					ServerMessage::TerminalAttached { .. }
						| ServerMessage::TerminalResized { .. }
				)
			),
		)
	}

	#[test]
	fn replies_are_classified_exactly_as_server_messages_are() {
		let error = WireError {
			category: ErrorCategory::Conflict,
			code: "test.refused".into(),
			retryable: false,
			message: "refused".into(),
			revision_conflict: None,
			restart: None,
			recovery_actions: vec![],
		};
		let error_json =
			String::from_utf8(encode_control(&error).unwrap()).unwrap();
		let mut payloads: Vec<_> = [
			ServerMessage::Error {
				id: None,
				error: error.clone(),
			},
			ServerMessage::Error { id: Some(1), error },
			ServerMessage::TerminalAttached { id: 2 },
			ServerMessage::TerminalResized { id: 3 },
			ServerMessage::RemoteToolResult {
				id: 4,
				result: RemoteToolResult::Written,
			},
			ServerMessage::QueryResult {
				id: 5,
				result: QueryResponse::Status(PlaneStatus {
					cursor: Some(0),
					plane_id: Uuid::nil(),
					daemon_starts: 1,
					started_at_unix_ms: 0,
					core_version: "0.2.0".into(),
					security: None,
					recovery: None,
				}),
			},
		]
		.iter()
		.map(|message| encode_control(message).unwrap())
		.collect();
		payloads.extend(
			[
				r#"{"kind":"terminal_attached","id":"2"}"#.to_owned(),
				r#"{"kind":"terminal_resized"}"#.to_owned(),
				r#"{"kind":"terminal_attached","id":2,"extra":true}"#.to_owned(),
				r#"{"id":3,"kind":"terminal_resized"}"#.to_owned(),
				r#"["terminal_attached",2]"#.to_owned(),
				r#"["terminal_resized"]"#.to_owned(),
				r#"{"kind":0,"id":2}"#.to_owned(),
				r#"{"kind":"terminal_attached","kind":"terminal_attached","id":2}"#.to_owned(),
				r#"{"kind":"Terminal_Attached","id":2}"#.to_owned(),
				r#"{"kind":"error","id":null,"error":{}}"#.to_owned(),
				format!(r#"{{"kind":"error","error":{error_json}}}"#),
				format!(r#"{{"kind":"error","id":null,"error":{error_json},"extra":[]}}"#),
				format!(r#"["error",null,{error_json}]"#),
				format!(r#"["error",1,{error_json}]"#),
				format!(r#"{{"kind":"error","id":"1","error":{error_json}}}"#),
				r#"{"kind":"query_result","id":1,"result":{}}"#.to_owned(),
				r#"{"type":"terminal_finished"}"#.to_owned(),
				"{}".to_owned(),
				"[]".to_owned(),
				"null".to_owned(),
			]
			.map(String::into_bytes),
		);

		let narrow: Vec<_> = payloads
			.iter()
			.map(|payload| class(decode_control(payload).ok()))
			.collect();
		let full: Vec<_> = payloads
			.iter()
			.map(|payload| full_class(decode_control(payload).ok()))
			.collect();

		assert_eq!(narrow, full);
	}
}
