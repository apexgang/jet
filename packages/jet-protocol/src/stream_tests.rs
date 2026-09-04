use pretty_assertions::assert_eq;

use super::{
	BinaryStreamKind, DataQueueOutcome, OutboundLimits, OutboundQueue,
	StreamQueueError,
};
use crate::{
	ControlError, ErrorCategory, Frame, FrameReader, FrameWriter,
	RecoveryAction, ServerMessage, StreamControl, StreamId, WireError,
	decode_control, encode_control,
};
use tokio::io::duplex;

fn stream(value: u32) -> StreamId {
	StreamId::new(value).unwrap()
}

#[test]
fn control_and_events_are_sent_before_credit_controlled_binary_data() {
	let mut queue = OutboundQueue::new(40);
	queue
		.open_binary(stream(7), BinaryStreamKind::Artifact)
		.unwrap();
	queue.grant_credit(stream(7), 3).unwrap();
	assert_eq!(
		queue.queue_data(stream(7), vec![1, 2, 3]).unwrap(),
		DataQueueOutcome::Queued
	);
	let bulk = Frame::data(stream(7), vec![1, 2, 3]);
	let event =
		Frame::stream_control(stream(5), br#"{"kind":"event"}"#.to_vec());
	queue.queue_event(41, event.clone()).unwrap();
	let reply =
		Frame::stream_control(stream(3), br#"{"kind":"reply"}"#.to_vec());
	queue.queue_control(reply.clone()).unwrap();

	assert_eq!(queue.next_frame(), Some(reply));
	queue.confirm_written();
	assert_eq!(queue.next_frame(), Some(event));
	queue.confirm_written();
	assert_eq!(queue.next_frame(), Some(bulk));
}

#[test]
fn binary_streams_cannot_advance_past_receiver_credit() {
	let mut queue = OutboundQueue::new(0);
	queue
		.open_binary(stream(1), BinaryStreamKind::Artifact)
		.unwrap();

	assert_eq!(
		queue.queue_data(stream(1), vec![1]).unwrap_err(),
		StreamQueueError::InsufficientCredit {
			stream_id: stream(1),
			available: 0,
			requested: 1,
		}
	);
	assert_eq!(queue.next_frame(), None);
}

#[test]
fn open_binary_streams_are_bounded_and_ids_can_be_reused_after_close() {
	let limits = OutboundLimits {
		open_streams: 1,
		..OutboundLimits::default()
	};
	let mut queue = OutboundQueue::with_limits(0, limits);
	queue
		.open_binary(stream(1), BinaryStreamKind::Terminal)
		.unwrap();

	assert_eq!(
		queue
			.open_binary(stream(2), BinaryStreamKind::Artifact)
			.unwrap_err(),
		StreamQueueError::TooManyStreams { limit: 1 }
	);
	queue.close_binary(stream(1)).unwrap();
	queue
		.open_binary(stream(2), BinaryStreamKind::Artifact)
		.unwrap();
}

#[test]
fn bounded_binary_queue_reports_terminal_gaps_but_backpressures_artifacts() {
	let limits = OutboundLimits {
		binary_bytes: 3,
		..OutboundLimits::default()
	};
	let mut queue = OutboundQueue::with_limits(0, limits);
	queue
		.open_binary(stream(1), BinaryStreamKind::Artifact)
		.unwrap();
	queue
		.open_binary(stream(2), BinaryStreamKind::Terminal)
		.unwrap();
	queue.grant_credit(stream(1), 8).unwrap();
	queue.grant_credit(stream(2), 8).unwrap();
	queue.queue_data(stream(1), vec![1, 2, 3]).unwrap();

	assert_eq!(
		queue.queue_data(stream(2), vec![4, 5]).unwrap(),
		DataQueueOutcome::TerminalGap {
			first_missing_offset: 0,
			missing_bytes: 2,
		}
	);
	assert_eq!(
		queue.queue_data(stream(1), vec![6]).unwrap_err(),
		StreamQueueError::Backpressured {
			stream_id: stream(1),
			queued_bytes: 3,
			limit: 3,
		}
	);

	let gap = queue.next_frame().unwrap();
	assert_eq!(gap.stream_id(), stream(2));
	assert_eq!(
		decode_control::<StreamControl>(gap.payload()).unwrap(),
		StreamControl::TerminalGap {
			first_missing_offset: 0,
			missing_bytes: 2,
		}
	);
	assert_eq!(
		queue.next_frame(),
		Some(Frame::data(stream(1), vec![1, 2, 3]))
	);
	assert_eq!(
		queue.queue_data(stream(2), vec![6, 7]).unwrap(),
		DataQueueOutcome::Queued
	);
}

#[test]
fn full_event_window_disconnects_with_the_last_delivered_cursor() {
	let limits = OutboundLimits {
		event_count: 1,
		..OutboundLimits::default()
	};
	let mut queue = OutboundQueue::with_limits(40, limits);
	queue
		.queue_event(41, Frame::stream_control(stream(5), vec![1]))
		.unwrap();
	assert!(queue.next_frame().is_some());
	queue.confirm_written();
	queue
		.queue_event(42, Frame::stream_control(stream(5), vec![2]))
		.unwrap();

	let error = queue
		.queue_event(43, Frame::stream_control(stream(5), vec![3]))
		.unwrap_err();
	assert_eq!(error, StreamQueueError::SlowConsumer { resume_after: 41 });
	assert_eq!(
		error.disconnect_error(),
		Some(WireError {
			category: ErrorCategory::Unavailable,
			code: "protocol.slow_consumer".into(),
			retryable: true,
			message: "the Event consumer exceeded its bounded window; reconnect and replay after the supplied cursor".into(),
			revision_conflict: None,
			restart: None,
			recovery_actions: vec![RecoveryAction::ResumeEvents { after: 41 }],
		})
	);
}

#[test]
fn an_event_is_not_recoverable_until_its_frame_write_succeeds() {
	let limits = OutboundLimits {
		event_count: 1,
		..OutboundLimits::default()
	};
	let mut queue = OutboundQueue::with_limits(40, limits);
	queue
		.queue_event(41, Frame::stream_control(stream(5), vec![1]))
		.unwrap();
	assert!(queue.next_frame().is_some());
	queue
		.queue_event(42, Frame::stream_control(stream(5), vec![2]))
		.unwrap();

	assert_eq!(
		queue
			.queue_event(43, Frame::stream_control(stream(5), vec![3]))
			.unwrap_err(),
		StreamQueueError::SlowConsumer { resume_after: 40 }
	);
}

#[test]
fn event_cursors_must_advance_strictly() {
	let mut queue = OutboundQueue::new(40);
	queue
		.queue_event(41, Frame::stream_control(stream(5), vec![1]))
		.unwrap();

	assert_eq!(
		queue
			.queue_event(41, Frame::stream_control(stream(5), vec![2]))
			.unwrap_err(),
		StreamQueueError::EventOutOfOrder {
			previous: 41,
			received: 41,
		}
	);
}

#[test]
fn stream_control_allows_optional_fields_but_rejects_unknown_variants() {
	let credit: StreamControl = decode_control(
		br#"{"type":"credit","bytes":8,"future_optional":true}"#,
	)
	.unwrap();
	assert_eq!(credit, StreamControl::Credit { bytes: 8 });

	let unknown = decode_control::<StreamControl>(
		br#"{"type":"replace_artifact","bytes":8}"#,
	)
	.unwrap_err();
	assert!(matches!(unknown, ControlError::Malformed(_)), "{unknown:?}");
}

#[test]
fn terminal_gap_has_an_exact_explicit_wire_shape() {
	let gap = StreamControl::TerminalGap {
		first_missing_offset: 9,
		missing_bytes: 4,
	};

	assert_eq!(
		String::from_utf8(encode_control(&gap).unwrap()).unwrap(),
		r#"{"type":"terminal_gap","first_missing_offset":"9","missing_bytes":"4"}"#
	);
}

#[tokio::test]
async fn terminal_gap_control_precedes_already_queued_artifact_bytes_on_wire() {
	let limits = OutboundLimits {
		binary_bytes: 3,
		..OutboundLimits::default()
	};
	let mut queue = OutboundQueue::with_limits(0, limits);
	queue
		.open_binary(stream(1), BinaryStreamKind::Artifact)
		.unwrap();
	queue
		.open_binary(stream(2), BinaryStreamKind::Terminal)
		.unwrap();
	queue.grant_credit(stream(1), 3).unwrap();
	queue.grant_credit(stream(2), 2).unwrap();
	queue.queue_data(stream(1), vec![1, 2, 3]).unwrap();
	queue.queue_data(stream(2), vec![4, 5]).unwrap();
	let (client, server) = duplex(128);
	let mut writer = FrameWriter::new(client);
	let mut reader = FrameReader::new(server);
	writer.enable_multiplexing();
	reader.enable_multiplexing();

	assert!(queue.write_next(&mut writer).await.unwrap());
	let Frame::Control { stream_id, payload } = reader.read().await.unwrap()
	else {
		panic!("expected explicit terminal-gap control");
	};
	assert_eq!(
		(
			stream_id,
			decode_control::<StreamControl>(&payload).unwrap()
		),
		(
			stream(2),
			StreamControl::TerminalGap {
				first_missing_offset: 0,
				missing_bytes: 2,
			}
		)
	);
	assert!(queue.write_next(&mut writer).await.unwrap());
	assert_eq!(
		reader.read().await.unwrap(),
		Frame::data(stream(1), vec![1, 2, 3])
	);
}

#[tokio::test]
async fn slow_consumer_error_is_sent_before_pending_event_with_written_cursor()
{
	let limits = OutboundLimits {
		event_count: 1,
		..OutboundLimits::default()
	};
	let mut queue = OutboundQueue::with_limits(40, limits);
	let (client, server) = duplex(512);
	let mut writer = FrameWriter::new(client);
	let mut reader = FrameReader::new(server);
	writer.enable_multiplexing();
	reader.enable_multiplexing();
	queue
		.queue_event(
			41,
			Frame::stream_control(stream(5), br#"{"kind":"event"}"#.to_vec()),
		)
		.unwrap();
	assert!(queue.write_next(&mut writer).await.unwrap());
	assert!(matches!(
		reader.read().await.unwrap(),
		Frame::Control { .. }
	));
	queue
		.queue_event(
			42,
			Frame::stream_control(stream(5), br#"{"kind":"event"}"#.to_vec()),
		)
		.unwrap();
	let error = queue
		.queue_event(
			43,
			Frame::stream_control(stream(5), br#"{"kind":"event"}"#.to_vec()),
		)
		.unwrap_err()
		.disconnect_error()
		.unwrap();
	queue
		.queue_control(Frame::control(
			encode_control(&ServerMessage::Error { id: None, error }).unwrap(),
		))
		.unwrap();

	assert!(queue.write_next(&mut writer).await.unwrap());
	let Frame::Control { stream_id, payload } = reader.read().await.unwrap()
	else {
		panic!("expected a connection-level slow-consumer error");
	};
	assert_eq!(stream_id, crate::CONNECTION_STREAM);
	let ServerMessage::Error { id: None, error } =
		decode_control(&payload).unwrap()
	else {
		panic!("expected a connection-level error");
	};
	assert_eq!(
		error.recovery_actions,
		vec![RecoveryAction::ResumeEvents { after: 41 }]
	);
}
