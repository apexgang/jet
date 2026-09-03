//! Black-box Command conformance tests at the public Jet protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	Actor, CODEC_JSON_V1, ClientHello, CommandRequest, CommandResponse,
	ConflictState, ErrorCategory, Frame, FrameReader, FrameWriter,
	MAX_CONTROL_FRAME, MAX_DATA_FRAME, PREFACE, RecoveryAction, Retention,
	RevisionConflict, RunLifecycle, ServerHello, ServerMessage, VersionRange,
	WireError, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use support::{connect, start_jetd};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use uuid::Uuid;

async fn raw_command(
	daemon: &support::Daemon,
	client_id: Uuid,
	command_id: Uuid,
	command: &str,
) -> ServerMessage {
	let mut stream = UnixStream::connect(&daemon.socket).await.unwrap();
	stream.write_all(PREFACE).await.unwrap();
	let (read, write) = stream.into_split();
	let mut writer = FrameWriter::new(write);
	let mut reader = FrameReader::new(read);
	writer
		.write(&Frame::Control(
			encode_control(&ClientHello {
				protocol: VersionRange { min: 1, max: 1 },
				codec: CODEC_JSON_V1.into(),
				client_id,
				max_control_frame: u32::try_from(MAX_CONTROL_FRAME).unwrap(),
				max_data_frame: u32::try_from(MAX_DATA_FRAME).unwrap(),
				capabilities: vec![],
			})
			.unwrap(),
		))
		.await
		.unwrap();
	let Frame::Control(welcome) = reader.read().await.unwrap() else {
		panic!("expected a control frame");
	};
	assert!(matches!(
		decode_control::<ServerHello>(&welcome).unwrap(),
		ServerHello::Welcome { .. }
	));
	writer
		.write(&Frame::Control(
			format!(
				"{{\"kind\":\"command\",\"id\":9,\"command_id\":\"{command_id}\",\"command\":{command}}}"
			)
			.into_bytes(),
		))
		.await
		.unwrap();
	let Frame::Control(reply) = reader.read().await.unwrap() else {
		panic!("expected a control frame");
	};
	decode_control(&reply).unwrap()
}

#[tokio::test]
async fn an_identical_retry_returns_the_durable_original_result() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let client_id = Uuid::new_v4();
	let command_id = Uuid::now_v7();
	let command = CommandRequest::CreateConversation {
		retention: Retention::Retain,
	};

	let mut first = start_jetd(&home).await;
	let original = connect(&first, client_id)
		.await
		.execute_command(command_id, command.clone())
		.await
		.unwrap();
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let mut client = connect(&second, client_id).await;
	let retried = client.execute_command(command_id, command).await.unwrap();
	let events = client.events_after(0).await.unwrap();

	assert_eq!(retried, original);
	assert!(matches!(original, CommandResponse::ConversationCreated(_)));
	assert_eq!(events.len(), 1);
	assert_eq!(events[0].actor, Actor::InteractiveClient { client_id });
}

#[tokio::test]
async fn changed_content_cannot_reuse_an_actors_command_identity() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut client = connect(&daemon, Uuid::new_v4()).await;
	let command_id = Uuid::now_v7();
	client
		.execute_command(
			command_id,
			CommandRequest::CreateConversation {
				retention: Retention::Retain,
			},
		)
		.await
		.unwrap();

	let error = client
		.execute_command(
			command_id,
			CommandRequest::CreateConversation {
				retention: Retention::ForgetAfterFinalRun,
			},
		)
		.await
		.unwrap_err();

	let jet_client::ClientError::Remote(error) = error else {
		panic!("expected a remote error, got {error:?}");
	};
	assert_eq!(
		error,
		WireError {
			category: ErrorCategory::Conflict,
			code: "command.identity_reused".into(),
			retryable: false,
			message:
				"the Command identity was already used for different content"
					.into(),
			revision_conflict: None,
			recovery_actions: vec![],
		}
	);
}

#[tokio::test]
async fn only_a_byte_equivalent_command_body_is_an_identical_retry() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client_id = Uuid::new_v4();
	let command_id = Uuid::now_v7();
	connect(&daemon, client_id)
		.await
		.execute_command(
			command_id,
			CommandRequest::CreateConversation {
				retention: Retention::Retain,
			},
		)
		.await
		.unwrap();

	let reply = raw_command(
		&daemon,
		client_id,
		command_id,
		r#"{ "type":"create_conversation","retention":"retain"}"#,
	)
	.await;

	assert_eq!(
		reply,
		ServerMessage::Error {
			id: Some(9),
			error: WireError {
				category: ErrorCategory::Conflict,
				code: "command.identity_reused".into(),
				retryable: false,
				message:
					"the Command identity was already used for different content"
						.into(),
				revision_conflict: None,
				recovery_actions: vec![],
			},
		}
	);
}

#[tokio::test]
async fn command_identities_are_scoped_to_the_authenticated_actor() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let first_actor = Uuid::new_v4();
	let second_actor = Uuid::new_v4();
	let command_id = Uuid::now_v7();
	let command = CommandRequest::CreateConversation {
		retention: Retention::Retain,
	};
	let mut first = connect(&daemon, first_actor).await;
	let mut second = connect(&daemon, second_actor).await;

	let first_result = first
		.execute_command(command_id, command.clone())
		.await
		.unwrap();
	let second_result =
		second.execute_command(command_id, command).await.unwrap();
	let actors = first
		.events_after(0)
		.await
		.unwrap()
		.into_iter()
		.map(|event| event.actor)
		.collect::<Vec<_>>();

	assert_ne!(first_result, second_result);
	assert_eq!(
		actors,
		vec![
			Actor::InteractiveClient {
				client_id: first_actor,
			},
			Actor::InteractiveClient {
				client_id: second_actor,
			},
		]
	);
}

#[tokio::test]
async fn concurrent_commands_expose_one_authoritative_revision_order() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let client_id = Uuid::new_v4();
	let mut setup = connect(&daemon, client_id).await;
	let conversation =
		setup.create_conversation(Retention::Retain).await.unwrap();
	let run = setup
		.create_run(conversation.conversation_id)
		.await
		.unwrap();
	let mut first = connect(&daemon, client_id).await;
	let mut second = connect(&daemon, client_id).await;

	let (first_result, second_result) = tokio::join!(
		first.execute_command(
			Uuid::now_v7(),
			CommandRequest::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: RunLifecycle::Starting,
			},
		),
		second.execute_command(
			Uuid::now_v7(),
			CommandRequest::TransitionRun {
				run_id: run.run_id,
				expected_revision: run.revision,
				lifecycle: RunLifecycle::Failed,
			},
		),
	);

	let (accepted, refused) = match (first_result, second_result) {
		(Ok(accepted), Err(refused)) | (Err(refused), Ok(accepted)) => {
			(accepted, refused)
		}
		other => {
			panic!("expected one accepted Command and one conflict: {other:?}")
		}
	};
	let CommandResponse::RunTransitioned(current) = accepted else {
		panic!("expected a transitioned Run");
	};
	let jet_client::ClientError::Remote(refused) = refused else {
		panic!("expected a remote conflict");
	};
	assert_eq!(
		refused,
		WireError {
			category: ErrorCategory::Conflict,
			code: "run.revision_conflict".into(),
			retryable: false,
			message: "the Run changed since the Command was prepared".into(),
			revision_conflict: Some(RevisionConflict {
				current_revision: 2,
				safe_state: ConflictState::Run {
					run: current.clone(),
				},
			}),
			recovery_actions: vec![RecoveryAction::RefreshRun {
				run_id: current.run_id,
			}],
		}
	);
	let snapshot = setup
		.conversation(conversation.conversation_id)
		.await
		.unwrap();
	assert_eq!(snapshot.runs, vec![current]);
	assert_eq!(
		setup
			.events_after(0)
			.await
			.unwrap()
			.into_iter()
			.map(|event| event.sequence)
			.collect::<Vec<_>>(),
		vec![1, 2, 3]
	);
}

#[tokio::test]
async fn retrying_a_rejected_command_returns_its_original_conflict() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = start_jetd(&dir.path().join(".jet")).await;
	let mut client = connect(&daemon, Uuid::new_v4()).await;
	let conversation =
		client.create_conversation(Retention::Retain).await.unwrap();
	let run = client
		.create_run(conversation.conversation_id)
		.await
		.unwrap();
	let starting = client
		.transition_run(run.run_id, run.revision, RunLifecycle::Starting)
		.await
		.unwrap();
	let command_id = Uuid::now_v7();
	let stale = CommandRequest::TransitionRun {
		run_id: run.run_id,
		expected_revision: run.revision,
		lifecycle: RunLifecycle::Failed,
	};

	let original = client
		.execute_command(command_id, stale.clone())
		.await
		.unwrap_err();
	let jet_client::ClientError::Remote(original) = original else {
		panic!("expected a remote conflict");
	};
	let active = client
		.transition_run(
			starting.run_id,
			starting.revision,
			RunLifecycle::Active,
		)
		.await
		.unwrap();
	let retried = client.execute_command(command_id, stale).await.unwrap_err();
	let jet_client::ClientError::Remote(retried) = retried else {
		panic!("expected a remote conflict");
	};

	assert_eq!(retried, original);
	assert_eq!(
		client
			.conversation(conversation.conversation_id)
			.await
			.unwrap()
			.runs,
		vec![active]
	);
}
