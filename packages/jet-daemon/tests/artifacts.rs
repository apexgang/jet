//! Artifact publication and binary transport through a real authenticated daemon.
mod support;
use jet_protocol::{
	ArtifactControl, ArtifactDescriptor, Frame, RetentionPolicy, StreamControl,
	StreamId,
};
use pretty_assertions::assert_eq;
use uuid::Uuid;
const ABC: &str =
	"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[tokio::test]
async fn download_credit_spans_bounded_chunks_and_corruption_stays_on_its_stream()
 {
	let home = tempfile::tempdir().unwrap();
	let daemon = support::start_jetd(home.path()).await;
	let client = support::connect(&daemon, Uuid::nil()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let artifact = ArtifactDescriptor {
		sha256: jet_protocol::Sha256Digest::parse(
			"d69e68988157833272305aaf21f453c800346e8a3640db6578e260215542e5d4",
		)
		.unwrap(),
		size: 100_000,
	};
	let source = vec![b'x'; artifact.size as usize];
	client
		.upload_artifact(run.run_id, artifact.clone(), &mut source.as_slice())
		.await
		.unwrap();
	let mut wire = support::connect_raw(&daemon, Uuid::nil()).await;
	wire.send(&ArtifactControl::ArtifactDownload {
		sha256: artifact.sha256,
	})
	.await;
	assert_eq!(
		wire.receive::<ArtifactControl>().await,
		ArtifactControl::ArtifactDownloading {
			artifact: artifact.clone()
		}
	);
	// Credit is an additional-byte window, independent of a single frame's limit.
	wire.send(&StreamControl::Credit { bytes: 1024 * 1024 })
		.await;
	let mut downloaded = Vec::new();
	while downloaded.len() < source.len() {
		let Frame::Data { stream_id, payload } = wire.receive_frame().await
		else {
			panic!("credited data")
		};
		assert_eq!(stream_id, StreamId::new(1).unwrap());
		assert!(!payload.is_empty() && payload.len() <= 65536);
		downloaded.extend(payload);
	}
	assert_eq!(downloaded, source);
	assert_eq!(
		wire.receive::<StreamControl>().await,
		StreamControl::ArtifactFinished {
			total_bytes: artifact.size,
			sha256: artifact.sha256
		}
	);

	std::fs::write(
		home.path()
			.join("artifacts/payloads")
			.join(artifact.sha256.to_string()),
		vec![b'y'; source.len()],
	)
	.unwrap();
	let mut partial = Vec::new();
	let error = client
		.download_artifact(artifact.sha256, &mut partial)
		.await
		.unwrap_err();
	assert!(
		matches!(error, jet_client::ClientError::Remote(e) if e.code == "artifact.corrupt")
	);
	// The stream error follows its data; a failed transfer cannot close this client.
	assert_eq!(partial, vec![b'y'; source.len()]);
	client.status().await.unwrap();
}

#[tokio::test]
async fn client_streams_artifacts_beside_queries_without_buffering_the_payload()
{
	let home = tempfile::tempdir().unwrap();
	let daemon = support::start_jetd(home.path()).await;
	let client = support::connect(&daemon, Uuid::nil()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let artifact = ArtifactDescriptor {
		sha256: jet_protocol::Sha256Digest::parse(ABC).unwrap(),
		size: 3,
	};
	let mut source = b"abc".as_slice();
	let (upload, status) = tokio::join!(
		client.upload_artifact(run.run_id, artifact.clone(), &mut source),
		client.status()
	);
	status.unwrap();
	assert_eq!(upload.unwrap(), artifact);
	let mut downloaded = Vec::new();
	assert_eq!(
		client
			.download_artifact(artifact.sha256, &mut downloaded)
			.await
			.unwrap(),
		artifact
	);
	assert_eq!(downloaded, b"abc");
	assert_eq!(client.collect_artifacts().await.unwrap(), 0);
	client
		.set_setting(
			Uuid::now_v7(),
			jet_protocol::SettingKey::ArtifactMaxMiB,
			jet_protocol::SettingScope::Plane,
			jet_protocol::SettingValue::Count(0),
		)
		.await
		.unwrap();
	let error = client
		.upload_artifact(run.run_id, artifact, &mut b"abc".as_slice())
		.await
		.unwrap_err();
	assert!(
		matches!(error, jet_client::ClientError::Remote(e) if e.code == "artifact.size_exceeded")
	);
}

#[tokio::test]
async fn bad_uploads_fail_on_the_wire_without_disrupting_other_streams() {
	let home = tempfile::tempdir().unwrap();
	let daemon = support::start_jetd(home.path()).await;
	let client = support::connect(&daemon, Uuid::nil()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let mut wire = support::connect_raw(&daemon, Uuid::nil()).await;
	let artifact = ArtifactDescriptor {
		sha256: jet_protocol::Sha256Digest::parse(ABC).unwrap(),
		size: 3,
	};
	for (bytes, code) in [
		(b"ab".as_slice(), "artifact.size_mismatch"),
		(b"bad".as_slice(), "artifact.hash_mismatch"),
	] {
		wire.send(&ArtifactControl::ArtifactUpload {
			run_id: run.run_id,
			artifact: artifact.clone(),
		})
		.await;
		let _: StreamControl = wire.receive().await;
		wire.send_frame(Frame::data(StreamId::new(1).unwrap(), bytes.to_vec()))
			.await;
		let _: StreamControl = wire.receive().await;
		wire.send(&ArtifactControl::ArtifactCommit).await;
		assert!(
			matches!(wire.receive::<jet_protocol::ServerMessage>().await, jet_protocol::ServerMessage::Error { error, .. } if error.code == code)
		);
	}
	wire.send(&ArtifactControl::ArtifactUpload {
		run_id: run.run_id,
		artifact: artifact.clone(),
	})
	.await;
	let _: StreamControl = wire.receive().await;
	wire.send(&ArtifactControl::ArtifactCancel).await;
	assert_eq!(
		wire.receive::<ArtifactControl>().await,
		ArtifactControl::ArtifactCanceled
	);
	wire.send(&ArtifactControl::ArtifactDownload {
		sha256: artifact.sha256,
	})
	.await;
	assert!(
		matches!(wire.receive::<jet_protocol::ServerMessage>().await, jet_protocol::ServerMessage::Error { error, .. } if error.code == "artifact.not_found")
	);
	client.status().await.unwrap();
}

#[tokio::test]
async fn binary_credit_and_negotiated_frame_limits_are_enforced() {
	let home = tempfile::tempdir().unwrap();
	let daemon = support::start_jetd(home.path()).await;
	let client = support::connect(&daemon, Uuid::nil()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let artifact = ArtifactDescriptor {
		sha256: jet_protocol::Sha256Digest::parse(ABC).unwrap(),
		size: 3,
	};
	let mut hello = support::hello(Uuid::nil());
	hello.max_data_frame = 2;
	let (mut wire, _) = support::handshake_raw(&daemon, &hello).await;
	wire.send(&ArtifactControl::ArtifactUpload {
		run_id: run.run_id,
		artifact: artifact.clone(),
	})
	.await;
	assert_eq!(
		wire.receive::<StreamControl>().await,
		StreamControl::Credit { bytes: 2 }
	);
	// The global frame parser permits three bytes, but this stream granted two.
	wire.send_frame(Frame::data(StreamId::new(1).unwrap(), b"abc".to_vec()))
		.await;
	assert!(
		matches!(wire.receive::<jet_protocol::ServerMessage>().await, jet_protocol::ServerMessage::Error { error, .. } if error.code == "artifact.invalid_stream")
	);
	let mut old = support::hello(Uuid::nil());
	old.minor = jet_protocol::ARTIFACTS_MINOR - 1;
	let (mut wire, _) = support::handshake_raw(&daemon, &old).await;
	wire.send(&ArtifactControl::ArtifactDownload {
		sha256: artifact.sha256,
	})
	.await;
	assert!(
		matches!(wire.receive::<jet_protocol::ServerMessage>().await, jet_protocol::ServerMessage::Error { error, .. } if error.code == "protocol.unsupported_minor")
	);
}

#[tokio::test]
async fn uploaded_artifact_is_downloaded_with_a_declaration_and_verified_completion()
 {
	let home = tempfile::tempdir().unwrap();
	let daemon = support::start_jetd(home.path()).await;
	let client = support::connect(&daemon, Uuid::nil()).await;
	let conversation = client
		.create_conversation(Uuid::now_v7(), RetentionPolicy::Retain)
		.await
		.unwrap();
	let run = client
		.create_run(Uuid::now_v7(), conversation.conversation_id)
		.await
		.unwrap();
	let mut wire = support::connect_raw(&daemon, Uuid::nil()).await;
	let artifact = ArtifactDescriptor {
		sha256: jet_protocol::Sha256Digest::parse(ABC).unwrap(),
		size: 3,
	};
	wire.send(&ArtifactControl::ArtifactUpload {
		run_id: run.run_id,
		artifact: artifact.clone(),
	})
	.await;
	assert!(
		matches!(wire.receive::<StreamControl>().await, StreamControl::Credit { bytes } if (3..=262144).contains(&bytes))
	);
	wire.send_frame(Frame::data(StreamId::new(1).unwrap(), b"abc".to_vec()))
		.await;
	assert_eq!(
		wire.receive::<StreamControl>().await,
		StreamControl::Credit { bytes: 3 }
	);
	// A contended filesystem publication must not block this connection's queries.
	let publication = std::fs::OpenOptions::new()
		.read(true)
		.write(true)
		.create(true)
		.truncate(false)
		.open(home.path().join("artifacts/payloads/.publication-lock"))
		.unwrap();
	publication.lock().unwrap();
	wire.send(&ArtifactControl::ArtifactCommit).await;
	wire.send_frame(Frame::stream_control(
		StreamId::new(2).unwrap(),
		jet_protocol::encode_control(&jet_protocol::ClientMessage::Query {
			id: 2,
			query: jet_protocol::QueryRequest::Status,
			timeout_ms: None,
		})
		.unwrap(),
	))
	.await;
	let status = tokio::time::timeout(
		std::time::Duration::from_secs(3),
		wire.receive::<jet_protocol::ServerMessage>(),
	)
	.await
	.expect("control query blocked behind Artifact publication");
	assert!(matches!(
		status,
		jet_protocol::ServerMessage::QueryResult {
			id: 2,
			result: jet_protocol::QueryResponse::Status(_)
		}
	));
	drop(publication);
	assert_eq!(
		wire.receive::<ArtifactControl>().await,
		ArtifactControl::ArtifactPublished {
			artifact: artifact.clone()
		}
	);
	wire.send(&ArtifactControl::ArtifactDownload {
		sha256: artifact.sha256,
	})
	.await;
	assert_eq!(
		wire.receive::<ArtifactControl>().await,
		ArtifactControl::ArtifactDownloading {
			artifact: artifact.clone()
		}
	);
	wire.send(&StreamControl::Credit { bytes: 3 }).await;
	assert_eq!(
		wire.receive_frame().await,
		Frame::data(StreamId::new(1).unwrap(), b"abc".to_vec())
	);
	assert_eq!(
		wire.receive::<StreamControl>().await,
		StreamControl::ArtifactFinished {
			total_bytes: 3,
			sha256: artifact.sha256
		}
	);
}
