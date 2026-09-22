use std::{path::PathBuf, sync::Arc, time::Duration};

use jet_client::{Client, ClientError};
use jet_protocol::{Event, PlaneStatus};
use uuid::Uuid;

#[cfg(test)]
use jet_protocol::{SettingKey, SettingScope};

use super::errors::PublicError;

#[derive(Clone)]
pub(crate) struct PlaneClient {
    socket: Arc<PathBuf>,
    client_id: Uuid,
    reconnect_delays: Arc<[Duration]>,
    event_poll_delay: Duration,
}

impl PlaneClient {
    pub(crate) fn new(
        socket: PathBuf,
        client_id: Uuid,
        reconnect_delays: impl Into<Arc<[Duration]>>,
        event_poll_delay: Duration,
    ) -> Self {
        Self {
            socket: Arc::new(socket),
            client_id,
            reconnect_delays: reconnect_delays.into(),
            event_poll_delay,
        }
    }

    pub(crate) async fn status(&self) -> Result<PlaneStatus, Box<ClientError>> {
        let mut attempt = 0;
        loop {
            let result = match self.connect().await {
                Ok(client) => client.status().await.map_err(Box::new),
                Err(error) => Err(error),
            };
            match result {
                Ok(status) => return Ok(status),
                Err(error) if reconnectable(&error) && attempt < self.reconnect_delays.len() => {
                    tokio::time::sleep(self.reconnect_delays[attempt]).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn clear_setting_with_retry(
        &self,
        command_id: Uuid,
        key: SettingKey,
        scope: SettingScope,
    ) -> Result<(), Box<ClientError>> {
        let mut attempt = 0;
        loop {
            let result = match self.connect().await {
                Ok(client) => client
                    .clear_setting(command_id, key, scope)
                    .await
                    .map_err(Box::new),
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => return Ok(()),
                Err(error) if reconnectable(&error) && attempt < self.reconnect_delays.len() => {
                    tokio::time::sleep(self.reconnect_delays[attempt]).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) async fn stream_updates<F>(&self, mut cursor: u64, mut send: F)
    where
        F: FnMut(NativeUpdate) -> bool,
    {
        let mut reconnect_attempt = 0usize;
        loop {
            let client = match self.connect().await {
                Ok(client) => client,
                Err(error) => {
                    let public = PublicError::from_client(&error);
                    if !send(failure_update(public.clone())) || !public.retryable {
                        return;
                    }
                    self.wait_to_reconnect(reconnect_attempt).await;
                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                    continue;
                }
            };

            let mut resumed = false;
            loop {
                match client.events_after(cursor).await {
                    Ok(page) => {
                        if !resumed {
                            if !send(NativeUpdate::Resumed { after: cursor }) {
                                return;
                            }
                            resumed = true;
                            reconnect_attempt = 0;
                        }
                        for event in page.events {
                            if event.sequence <= cursor {
                                let error = PublicError::invalid_event_order();
                                let _ = send(NativeUpdate::Failed { error });
                                return;
                            }
                            cursor = event.sequence;
                            if !send(NativeUpdate::Event(EventSummary::from_event(event))) {
                                return;
                            }
                        }
                        if cursor >= page.cursor {
                            tokio::time::sleep(self.event_poll_delay).await;
                        }
                    }
                    Err(error) => {
                        let public = PublicError::from_client(&error);
                        if !reconnectable(&error) {
                            let _ = send(NativeUpdate::Failed { error: public });
                            return;
                        }
                        if !send(NativeUpdate::Reconnecting { error: public }) {
                            return;
                        }
                        self.wait_to_reconnect(reconnect_attempt).await;
                        reconnect_attempt = reconnect_attempt.saturating_add(1);
                        break;
                    }
                }
            }
        }
    }

    async fn connect(&self) -> Result<Client, Box<ClientError>> {
        Client::connect_local(self.socket.as_ref(), self.client_id)
            .await
            .map_err(Box::new)
    }

    async fn wait_to_reconnect(&self, attempt: usize) {
        let delay = self
            .reconnect_delays
            .get(attempt)
            .or_else(|| self.reconnect_delays.last())
            .copied()
            .unwrap_or(Duration::from_millis(100));
        tokio::time::sleep(delay).await;
    }
}

fn reconnectable(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io(_)
            | ClientError::Frame(_)
            | ClientError::Control(_)
            | ClientError::Disconnected(_)
            | ClientError::Closed
    )
}

fn failure_update(error: PublicError) -> NativeUpdate {
    if error.retryable {
        NativeUpdate::Reconnecting { error }
    } else {
        NativeUpdate::Failed { error }
    }
}

pub(crate) enum NativeUpdate {
    Resumed { after: u64 },
    Event(EventSummary),
    Reconnecting { error: PublicError },
    Failed { error: PublicError },
}

pub(crate) struct EventSummary {
    pub(crate) sequence: u64,
    pub(crate) recorded_at_unix_ms: i64,
    pub(crate) kind: String,
}

impl EventSummary {
    fn from_event(event: Event) -> Self {
        Self {
            sequence: event.sequence,
            recorded_at_unix_ms: event.recorded_at_unix_ms,
            kind: safe_event_kind(&event.kind),
        }
    }
}

fn safe_event_kind(value: &str) -> String {
    // ASVS 2.1.1 and 2.2.1: render only a bounded identifier. Event payload,
    // actor, origin, and entity identifiers never cross into the webview.
    if !value.is_empty()
        && value.len() <= 80
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        value.to_owned()
    } else {
        "unknown".into()
    }
}

#[cfg(test)]
mod unit_tests {
    use std::time::Duration;

    use jet_protocol::{
        decode_control, encode_control, Actor, ClientHello, ClientMessage, CommandRequest,
        CommandResponse, Event, EventPage, Frame, FrameReader, FrameWriter, PlaneStatus,
        QueryRequest, QueryResponse, ServerHello, ServerMessage, SettingKey, SettingScope,
        StreamId,
    };
    use tokio::{
        io::AsyncReadExt,
        net::{unix::OwnedReadHalf, unix::OwnedWriteHalf, UnixListener},
    };
    use uuid::Uuid;

    use super::{safe_event_kind, NativeUpdate, PlaneClient};

    #[test]
    fn event_kind_is_bounded_before_it_reaches_the_webview() {
        assert_eq!(
            safe_event_kind("run.lifecycle_changed"),
            "run.lifecycle_changed"
        );
        assert_eq!(safe_event_kind("<img src=x onerror=alert(1)>"), "unknown");
    }

    #[tokio::test]
    async fn query_command_stream_reconnect_and_cursor_resume_share_one_client_path() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("jetd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let client_id = Uuid::from_u128(7);
        let command_id = Uuid::from_u128(8);

        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = accept(&listener, client_id).await;
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                message,
                ClientMessage::Query {
                    query: QueryRequest::Status,
                    ..
                }
            ));
            let id = request_id(&message);
            reply(
                &mut writer,
                stream,
                ServerMessage::QueryResult {
                    id,
                    result: QueryResponse::Status(status(10)),
                },
            )
            .await;
            drop((reader, writer));

            let (mut reader, writer) = accept(&listener, client_id).await;
            let (_, first_command) = next_message(&mut reader).await;
            assert_clear_setting(&first_command, command_id);
            drop((reader, writer));

            let (mut reader, mut writer) = accept(&listener, client_id).await;
            let (stream, retried_command) = next_message(&mut reader).await;
            assert_clear_setting(&retried_command, command_id);
            let id = request_id(&retried_command);
            reply(
                &mut writer,
                stream,
                ServerMessage::CommandResult {
                    id,
                    result: CommandResponse::SettingCleared {
                        key: SettingKey::EnergyConstrained,
                        scope: SettingScope::Plane,
                    },
                },
            )
            .await;
            drop((reader, writer));

            let (mut reader, mut writer) = accept(&listener, client_id).await;
            let (stream, first_events) = next_message(&mut reader).await;
            assert_events_after(&first_events, 10);
            let id = request_id(&first_events);
            reply(
                &mut writer,
                stream,
                ServerMessage::QueryResult {
                    id,
                    result: QueryResponse::Events(EventPage {
                        cursor: 11,
                        events: vec![event(11)],
                    }),
                },
            )
            .await;
            let (_, next_page) = next_message(&mut reader).await;
            assert_events_after(&next_page, 11);
            drop((reader, writer));

            let (mut reader, mut writer) = accept(&listener, client_id).await;
            let (stream, resumed_events) = next_message(&mut reader).await;
            assert_events_after(&resumed_events, 11);
            let id = request_id(&resumed_events);
            reply(
                &mut writer,
                stream,
                ServerMessage::QueryResult {
                    id,
                    result: QueryResponse::Events(EventPage {
                        cursor: 12,
                        events: vec![event(12)],
                    }),
                },
            )
            .await;
        });

        let client = PlaneClient::new(
            socket,
            client_id,
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        );
        assert_eq!(client.status().await.unwrap().cursor, Some(10));
        client
            .clear_setting_with_retry(
                command_id,
                SettingKey::EnergyConstrained,
                SettingScope::Plane,
            )
            .await
            .unwrap();

        let mut sequences = Vec::new();
        client
            .stream_updates(10, |update| {
                if let NativeUpdate::Event(event) = update {
                    sequences.push(event.sequence);
                }
                sequences.len() < 2
            })
            .await;
        assert_eq!(sequences, vec![11, 12]);
        server.await.unwrap();
    }

    async fn accept(
        listener: &UnixListener,
        expected_client_id: Uuid,
    ) -> (FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>) {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut preface = vec![0; jet_protocol::PREFACE.len()];
        stream.read_exact(&mut preface).await.unwrap();
        assert_eq!(preface, jet_protocol::PREFACE);
        let (read, write) = stream.into_split();
        let mut reader = FrameReader::new(read);
        let mut writer = FrameWriter::new(write);
        let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
            panic!("expected a control-frame hello");
        };
        let hello: ClientHello = decode_control(&payload).unwrap();
        assert_eq!(hello.client_id, expected_client_id);
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
        (reader, writer)
    }

    async fn next_message(reader: &mut FrameReader<OwnedReadHalf>) -> (StreamId, ClientMessage) {
        let Frame::Control { stream_id, payload } = reader.read().await.unwrap() else {
            panic!("expected a control-frame request");
        };
        (stream_id, decode_control(&payload).unwrap())
    }

    async fn reply(
        writer: &mut FrameWriter<OwnedWriteHalf>,
        stream: StreamId,
        message: ServerMessage,
    ) {
        writer
            .write(&Frame::stream_control(
                stream,
                encode_control(&message).unwrap(),
            ))
            .await
            .unwrap();
    }

    fn request_id(message: &ClientMessage) -> u64 {
        match message {
            ClientMessage::Query { id, .. } | ClientMessage::Command { id, .. } => *id,
            other => panic!("unexpected request: {other:?}"),
        }
    }

    fn assert_clear_setting(message: &ClientMessage, expected_command_id: Uuid) {
        assert!(matches!(
            message,
            ClientMessage::Command {
                command_id,
                command: CommandRequest::ClearSetting {
                    key: SettingKey::EnergyConstrained,
                    scope: SettingScope::Plane,
                },
                ..
            } if *command_id == expected_command_id
        ));
    }

    fn assert_events_after(message: &ClientMessage, expected_cursor: u64) {
        assert!(matches!(
            message,
            ClientMessage::Query {
                query: QueryRequest::Events { after },
                ..
            } if *after == expected_cursor
        ));
    }

    fn status(cursor: u64) -> PlaneStatus {
        PlaneStatus {
            cursor: Some(cursor),
            plane_id: Uuid::from_u128(1),
            daemon_starts: 3,
            started_at_unix_ms: 1_700_000_000_000,
            core_version: "0.2.0".into(),
            security: None,
            recovery: None,
        }
    }

    fn event(sequence: u64) -> Event {
        Event {
            sequence,
            event_id: Uuid::from_u128(u128::from(sequence)),
            actor: Actor::InteractiveClient {
                client_id: Uuid::from_u128(7),
            },
            origin: None,
            recorded_at_unix_ms: i64::try_from(sequence).unwrap(),
            conversation_id: None,
            run_id: None,
            kind: "test.event".into(),
            payload_version: 1,
            payload: serde_json::json!({ "private": "not forwarded" }),
        }
    }
}
