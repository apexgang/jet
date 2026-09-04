use jet_protocol::{
	ClientHello, Frame, FrameReader, FrameWriter, PageCursor, ServerHello,
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
		let Frame::Control(hello) = reader.read().await.unwrap() else {
			panic!("expected a control frame");
		};
		let hello: ClientHello = decode_control(&hello).unwrap();
		assert_eq!(hello.protocol, VersionRange { min: 1, max: 1 });
		writer
			.write(&Frame::Control(
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
		let Frame::Control(_) = reader.read().await.unwrap() else {
			panic!("expected the status Query");
		};
		writer
			.write(&Frame::Control(
				br#"{"kind":"query_result","id":1,"result":{"type":"status","plane_id":"00000000-0000-0000-0000-000000000000","daemon_starts":1,"started_at_unix_ms":0,"core_version":"0.1.0"}}"#
					.to_vec(),
			))
			.await
			.unwrap();
	});

	let mut client = Client::connect_local(&socket, Uuid::nil()).await.unwrap();
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
