use pretty_assertions::assert_eq;
use tokio::io::{AsyncWriteExt, duplex};

use super::{
	Frame, FrameError, FrameKind, FrameLimits, FrameReader, FrameWriter,
	MAX_CONTROL_FRAME, MAX_DATA_FRAME,
};

#[tokio::test]
async fn control_and_data_frames_round_trip_in_order() {
	let (client, server) = duplex(4096);
	let mut writer = FrameWriter::new(client);
	let mut reader = FrameReader::new(server);

	writer
		.write(&Frame::Control(b"{\"kind\":\"ping\"}".to_vec()))
		.await
		.unwrap();
	writer.write(&Frame::Data(vec![0, 255, 7])).await.unwrap();

	let first = reader.read().await.unwrap();
	let second = reader.read().await.unwrap();
	assert_eq!(
		(first, second),
		(
			Frame::Control(b"{\"kind\":\"ping\"}".to_vec()),
			Frame::Data(vec![0, 255, 7])
		)
	);
}

#[tokio::test]
async fn oversized_control_declaration_is_rejected_before_payload_arrives() {
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	let declared = u32::try_from(MAX_CONTROL_FRAME + 1).unwrap();
	let mut header = vec![FrameKind::Control as u8];
	header.extend_from_slice(&declared.to_be_bytes());
	client.write_all(&header).await.unwrap();

	let error = reader.read().await.unwrap_err();
	assert_eq!(
		error,
		FrameError::Oversized {
			kind: FrameKind::Control,
			declared: MAX_CONTROL_FRAME + 1,
			limit: MAX_CONTROL_FRAME,
		}
	);
}

#[tokio::test]
async fn oversized_data_declaration_is_rejected() {
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	let declared = u32::try_from(MAX_DATA_FRAME + 1).unwrap();
	let mut header = vec![FrameKind::Data as u8];
	header.extend_from_slice(&declared.to_be_bytes());
	client.write_all(&header).await.unwrap();

	let error = reader.read().await.unwrap_err();
	assert_eq!(
		error,
		FrameError::Oversized {
			kind: FrameKind::Data,
			declared: MAX_DATA_FRAME + 1,
			limit: MAX_DATA_FRAME,
		}
	);
}

#[tokio::test]
async fn unknown_frame_kind_is_rejected() {
	let (mut client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	client.write_all(&[9, 0, 0, 0, 0]).await.unwrap();

	let error = reader.read().await.unwrap_err();
	assert_eq!(error, FrameError::UnknownKind(9));
}

#[tokio::test]
async fn writer_refuses_oversized_frames() {
	let (client, _server) = duplex(64);
	let mut writer = FrameWriter::new(client);

	let error = writer
		.write(&Frame::Data(vec![0; MAX_DATA_FRAME + 1]))
		.await
		.unwrap_err();
	assert_eq!(
		error,
		FrameError::Oversized {
			kind: FrameKind::Data,
			declared: MAX_DATA_FRAME + 1,
			limit: MAX_DATA_FRAME,
		}
	);
}

#[tokio::test]
async fn writer_honors_negotiated_limits_below_the_protocol_maxima() {
	let (client, _server) = duplex(64);
	let mut writer = FrameWriter::new(client);
	writer.set_limits(FrameLimits::default().negotiate(FrameLimits {
		control: 16,
		data: 8,
	}));

	let error = writer
		.write(&Frame::Control(vec![b'{'; 17]))
		.await
		.unwrap_err();
	assert_eq!(
		error,
		FrameError::Oversized {
			kind: FrameKind::Control,
			declared: 17,
			limit: 16,
		}
	);
}

#[tokio::test]
async fn closed_peer_reports_end_of_stream() {
	let (client, server) = duplex(64);
	let mut reader = FrameReader::new(server);
	drop(client);

	let error = reader.read().await.unwrap_err();
	assert_eq!(error, FrameError::Closed);
}
