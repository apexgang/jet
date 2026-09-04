use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

use super::{
	CONNECTION_STREAM, Frame, FrameError, FrameKind, FrameLimits, FrameReader,
	FrameWriter, MAX_CONTROL_FRAME, MAX_DATA_FRAME, StreamId,
};

/// The `(kind, declared, limit)` of an oversized-frame error.
fn oversized(error: &FrameError) -> (FrameKind, usize, usize) {
	let FrameError::Oversized {
		kind,
		declared,
		limit,
	} = error
	else {
		panic!("expected an oversized frame error, got {error:?}");
	};
	(*kind, *declared, *limit)
}

#[tokio::test]
async fn control_and_data_frames_round_trip_in_order() {
	let (client, server) = duplex(4096);
	let mut writer = FrameWriter::new(client);
	let mut reader = FrameReader::new(server);
	writer.enable_multiplexing();
	reader.enable_multiplexing();

	writer
		.write(&Frame::control(b"{\"kind\":\"ping\"}".to_vec()))
		.await
		.unwrap();
	writer
		.write(&Frame::data(StreamId::new(7).unwrap(), vec![0, 255, 7]))
		.await
		.unwrap();

	let first = reader.read().await.unwrap();
	let second = reader.read().await.unwrap();
	assert_eq!(
		(first, second),
		(
			Frame::control(b"{\"kind\":\"ping\"}".to_vec()),
			Frame::data(StreamId::new(7).unwrap(), vec![0, 255, 7])
		)
	);
}

#[tokio::test]
async fn envelope_carries_kind_stream_and_length_in_network_order() {
	let (client, mut server) = duplex(64);
	let mut writer = FrameWriter::new(client);
	writer.enable_multiplexing();
	writer
		.write(&Frame::data(
			StreamId::new(0x0102_0304).unwrap(),
			vec![5, 6],
		))
		.await
		.unwrap();

	let mut encoded = [0u8; 11];
	server.read_exact(&mut encoded).await.unwrap();
	assert_eq!(encoded, [1, 1, 2, 3, 4, 0, 0, 0, 2, 5, 6]);
}

#[tokio::test]
async fn connection_control_uses_the_legacy_envelope_until_negotiation() {
	let (client, mut server) = duplex(64);
	let mut writer = FrameWriter::new(client);
	writer.write(&Frame::control(vec![5, 6])).await.unwrap();

	let mut encoded = [0u8; 7];
	server.read_exact(&mut encoded).await.unwrap();
	assert_eq!(encoded, [0, 0, 0, 0, 2, 5, 6]);
}

#[tokio::test]
async fn oversized_control_declaration_is_rejected_before_payload_arrives() {
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	reader.enable_multiplexing();
	let declared = u32::try_from(MAX_CONTROL_FRAME + 1).unwrap();
	let mut header = vec![FrameKind::Control as u8];
	header.extend_from_slice(&CONNECTION_STREAM.get().to_be_bytes());
	header.extend_from_slice(&declared.to_be_bytes());
	client.write_all(&header).await.unwrap();

	let error = reader.read().await.unwrap_err();
	assert_eq!(
		oversized(&error),
		(FrameKind::Control, MAX_CONTROL_FRAME + 1, MAX_CONTROL_FRAME)
	);
}

#[tokio::test]
async fn oversized_data_declaration_is_rejected() {
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	reader.enable_multiplexing();
	let declared = u32::try_from(MAX_DATA_FRAME + 1).unwrap();
	let mut header = vec![FrameKind::Data as u8];
	header.extend_from_slice(&StreamId::new(1).unwrap().get().to_be_bytes());
	header.extend_from_slice(&declared.to_be_bytes());
	client.write_all(&header).await.unwrap();

	let error = reader.read().await.unwrap_err();
	assert_eq!(
		oversized(&error),
		(FrameKind::Data, MAX_DATA_FRAME + 1, MAX_DATA_FRAME)
	);
}

#[tokio::test]
async fn unknown_frame_kind_is_rejected() {
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	reader.enable_multiplexing();
	client
		.write_all(&[9, 0, 0, 0, 0, 0, 0, 0, 0])
		.await
		.unwrap();

	let error = reader.read().await.unwrap_err();
	assert!(matches!(error, FrameError::UnknownKind(9)), "{error:?}");
}

#[tokio::test]
async fn writer_refuses_oversized_frames() {
	let (client, _server) = duplex(64);
	let mut writer = FrameWriter::new(client);
	writer.enable_multiplexing();

	let error = writer
		.write(&Frame::data(
			StreamId::new(1).unwrap(),
			vec![0; MAX_DATA_FRAME + 1],
		))
		.await
		.unwrap_err();
	assert_eq!(
		oversized(&error),
		(FrameKind::Data, MAX_DATA_FRAME + 1, MAX_DATA_FRAME)
	);
}

#[tokio::test]
async fn writer_honors_negotiated_limits_below_the_protocol_maxima() {
	let (client, _server) = duplex(64);
	let mut writer = FrameWriter::new(client);
	writer.enable_multiplexing();
	writer.set_limits(FrameLimits::default().negotiate(FrameLimits {
		control: 16,
		data: 8,
	}));

	let error = writer
		.write(&Frame::control(vec![b'{'; 17]))
		.await
		.unwrap_err();
	assert_eq!(oversized(&error), (FrameKind::Control, 17, 16));
}

#[tokio::test]
async fn data_on_the_reserved_connection_stream_is_rejected_before_allocation()
{
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	reader.enable_multiplexing();
	let mut header = vec![FrameKind::Data as u8];
	header.extend_from_slice(&CONNECTION_STREAM.get().to_be_bytes());
	header.extend_from_slice(&(MAX_DATA_FRAME as u32).to_be_bytes());
	client.write_all(&header).await.unwrap();

	let error = reader.read().await.unwrap_err();
	assert!(
		matches!(
			error,
			FrameError::InvalidStream {
				kind: FrameKind::Data,
				stream_id: CONNECTION_STREAM
			}
		),
		"{error:?}"
	);
}

#[tokio::test]
async fn closed_peer_reports_end_of_stream() {
	let (client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	drop(client);

	let error = reader.read().await.unwrap_err();
	assert!(matches!(error, FrameError::Closed), "{error:?}");
}

#[tokio::test]
async fn arbitrary_framed_bytes_terminate_without_panicking_or_waiting() {
	for seed in 0u32..256 {
		let mut state = seed.wrapping_add(1);
		let length = usize::try_from(seed % 65).unwrap();
		let mut bytes = Vec::with_capacity(length);
		for _ in 0..length {
			state ^= state << 13;
			state ^= state >> 17;
			state ^= state << 5;
			bytes.push(state.to_le_bytes()[0]);
		}
		let (mut client, server) = duplex(128);
		client.write_all(&bytes).await.unwrap();
		client.shutdown().await.unwrap();
		let mut reader = FrameReader::new(server);
		if seed % 2 == 0 {
			reader.enable_multiplexing();
		}
		let completed = tokio::time::timeout(
			std::time::Duration::from_millis(50),
			reader.read(),
		)
		.await;
		assert!(completed.is_ok(), "parser waited for seed {seed}");
	}
}
