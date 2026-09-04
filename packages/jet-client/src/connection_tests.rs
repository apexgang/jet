use jet_protocol::{
	ClientHello, ClientMessage, Frame, FrameReader, FrameWriter, PageCursor,
	PlaneStatus, QueryRequest, QueryResponse, ServerHello, ServerMessage,
	VersionRange, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;
use uuid::Uuid;

use super::{Client, ClientError};

#[tokio::test]
async fn a_new_client_keeps_the_minor_zero_contract_with_an_old_daemon() {
	let dir = tempfile::tempdir().unwrap();
	let socket = dir.path().join("old-jetd.sock");
	let listener = UnixListener::bind(&socket).unwrap();
	let server = tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut preface = vec![0; jet_protocol::PREFACE.len()];
		stream.read_exact(&mut preface).await.unwrap();
		assert_eq!(preface, jet_protocol::PREFACE);
		let (read, write) = stream.into_split();
		let mut reader = FrameReader::new(read);
		let mut writer = FrameWriter::new(write);
		let Frame::Control { payload: hello, .. } =
			reader.read().await.unwrap()
		else {
			panic!("expected a control frame");
		};
		let hello: ClientHello = decode_control(&hello).unwrap();
		assert_eq!(hello.protocol, VersionRange { min: 1, max: 1 });
		writer
			.write(&Frame::control(
				encode_control(&ServerHello::Welcome {
					protocol: 1,
					minor: 0,
					codec: "json-v1".into(),
					max_control_frame: 1_048_576,
					max_data_frame: 262_144,
					capabilities: vec![],
				})
				.unwrap(),
			))
			.await
			.unwrap();
		let Frame::Control { .. } = reader.read().await.unwrap() else {
			panic!("expected the status Query");
		};
		writer
			.write(&Frame::control(
				br#"{"kind":"query_result","id":1,"result":{"type":"status","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":1,"started_at_unix_ms":0,"core_version":"0.1.0"}}"#
					.to_vec(),
			))
			.await
			.unwrap();
	});

	let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
	let status = client.status().await.unwrap();
	let unavailable = client
		.next_conversations(PageCursor(Uuid::nil()))
		.await
		.unwrap_err();

	assert_eq!(status.cursor, None);
	assert!(matches!(
		unavailable,
		ClientError::FeatureUnavailable {
			required_minor: 1,
			negotiated_minor: 0
		}
	));
	server.await.unwrap();
}

#[tokio::test]
async fn a_current_client_switches_to_numbered_streams_after_the_handshake() {
	let dir = tempfile::tempdir().unwrap();
	let socket = dir.path().join("jetd.sock");
	let listener = UnixListener::bind(&socket).unwrap();
	let server = tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut preface = vec![0; jet_protocol::PREFACE.len()];
		stream.read_exact(&mut preface).await.unwrap();
		let (read, write) = stream.into_split();
		let mut reader = FrameReader::new(read);
		let mut writer = FrameWriter::new(write);
		let Frame::Control { .. } = reader.read().await.unwrap() else {
			panic!("expected a control-frame hello");
		};
		writer
			.write(&Frame::control(
				encode_control(&ServerHello::Welcome {
					protocol: jet_protocol::PROTOCOL_VERSION,
					minor: jet_protocol::PROTOCOL_MINOR,
					codec: jet_protocol::CODEC_JSON_V1.into(),
					max_control_frame: 1_048_576,
					max_data_frame: 262_144,
					capabilities: vec![],
				})
				.unwrap(),
			))
			.await
			.unwrap();
		reader.enable_multiplexing();
		writer.enable_multiplexing();

		let Frame::Control { stream_id, payload } =
			reader.read().await.unwrap()
		else {
			panic!("expected a control-frame Query");
		};
		let request: jet_protocol::ClientMessage =
			decode_control(&payload).unwrap();
		assert!(matches!(
			request,
			jet_protocol::ClientMessage::Query {
				id: 1,
				query: QueryRequest::Status,
			}
		));
		assert!(!stream_id.is_connection());
		writer
			.write(&Frame::stream_control(
				stream_id,
				encode_control(&ServerMessage::QueryResult {
					id: 1,
					result: QueryResponse::Status(PlaneStatus {
						cursor: Some(0),
						plane_id: Uuid::nil(),
						daemon_starts: 1,
						started_at_unix_ms: 0,
						core_version: "0.2.0".into(),
					}),
				})
				.unwrap(),
			))
			.await
			.unwrap();
	});

	let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
	let status = client.status().await.unwrap();
	assert_eq!(
		status,
		PlaneStatus {
			cursor: Some(0),
			plane_id: Uuid::nil(),
			daemon_starts: 1,
			started_at_unix_ms: 0,
			core_version: "0.2.0".into(),
		}
	);
	server.await.unwrap();
}

#[tokio::test]
async fn concurrent_requests_are_demultiplexed_by_numbered_stream() {
	let dir = tempfile::tempdir().unwrap();
	let socket = dir.path().join("multiplexed-jetd.sock");
	let listener = UnixListener::bind(&socket).unwrap();
	let server = tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut preface = vec![0; jet_protocol::PREFACE.len()];
		stream.read_exact(&mut preface).await.unwrap();
		let (read, write) = stream.into_split();
		let mut reader = FrameReader::new(read);
		let mut writer = FrameWriter::new(write);
		assert!(matches!(
			reader.read().await.unwrap(),
			Frame::Control { .. }
		));
		writer
			.write(&Frame::control(
				encode_control(&ServerHello::Welcome {
					protocol: jet_protocol::PROTOCOL_VERSION,
					minor: jet_protocol::PROTOCOL_MINOR,
					codec: jet_protocol::CODEC_JSON_V1.into(),
					max_control_frame: 1_048_576,
					max_data_frame: 262_144,
					capabilities: vec![],
				})
				.unwrap(),
			))
			.await
			.unwrap();
		reader.enable_multiplexing();
		writer.enable_multiplexing();

		let mut requests = Vec::new();
		for _ in 0..2 {
			let Frame::Control { stream_id, payload } =
				reader.read().await.unwrap()
			else {
				panic!("expected a control-frame Query");
			};
			let ClientMessage::Query {
				id,
				query: QueryRequest::Status,
			} = decode_control(&payload).unwrap()
			else {
				panic!("expected a status Query");
			};
			requests.push((stream_id, id));
		}
		for (stream_id, id) in requests.into_iter().rev() {
			writer
				.write(&Frame::stream_control(
					stream_id,
					encode_control(&ServerMessage::QueryResult {
						id,
						result: QueryResponse::Status(PlaneStatus {
							cursor: Some(id),
							plane_id: Uuid::nil(),
							daemon_starts: id,
							started_at_unix_ms: 0,
							core_version: format!("reply-{id}"),
						}),
					})
					.unwrap(),
				))
				.await
				.unwrap();
		}
	});

	let client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
	let (first, second) = tokio::join!(client.status(), client.status());
	assert_eq!(first.unwrap().core_version, "reply-1");
	assert_eq!(second.unwrap().core_version, "reply-2");
	server.await.unwrap();
}
