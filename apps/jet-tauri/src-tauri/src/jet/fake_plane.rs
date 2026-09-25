//! A scripted Plane for shell tests: a Unix socket that speaks the Jet
//! protocol, registered either as a remote Plane handle on a manual deadline
//! clock, or as this computer's local Plane. Nothing leaves the process.

use std::{sync::Arc, time::Duration};

use jet_protocol::{
    encode_control, ClientMessage, CommandResponse, ErrorCategory, Frame, FrameReader, FrameWriter,
    QueryResponse, ServerHello, ServerMessage, StreamId, WireError,
};
use tokio::net::{
    unix::{OwnedReadHalf, OwnedWriteHalf},
    UnixListener,
};
use uuid::Uuid;

use super::{
    client::{
        unit_tests::{accept, next_message, reply, request_id},
        PlaneClient,
    },
    deadline::{manual::ManualTimer, Deadlines},
    enrollment::tests::{setup, Setup},
    planes::PlaneId,
    JetBridge,
};

pub(crate) type Reader = FrameReader<OwnedReadHalf>;
pub(crate) type Writer = FrameWriter<OwnedWriteHalf>;

pub(crate) struct FakePlane {
    pub(crate) setup: Setup,
    /// The webview's handle for this Plane.
    pub(crate) plane_id: String,
    pub(crate) plane: PlaneId,
    pub(crate) listener: UnixListener,
    pub(crate) clock: Arc<ManualTimer>,
    client_id: Uuid,
}

/// A remote Plane handle served by a local socket, on a manual clock.
pub(crate) fn remote_plane() -> FakePlane {
    let setup = setup();
    let socket = setup.directory.path().join("fake-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0xfa4e);
    let clock = Arc::new(ManualTimer::default());
    let client_id = setup.bridge.local().client_id();
    setup.bridge.planes.insert_for_test(
        id,
        "Build box",
        PlaneClient::new(
            socket,
            client_id,
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        )
        .with_deadlines(Deadlines::on(clock.clone())),
    );
    FakePlane {
        setup,
        plane_id: id.to_string(),
        plane: PlaneId::Remote(id),
        listener,
        clock,
        client_id,
    }
}

/// This computer's local Plane: the test bridge's local socket, served.
/// Its deadlines run on the real clock and are never reached.
pub(crate) fn local_plane() -> FakePlane {
    let setup = setup();
    let socket = setup.bridge.local().socket_for_test().to_owned();
    let listener = UnixListener::bind(socket).unwrap();
    let client_id = setup.bridge.local().client_id();
    FakePlane {
        setup,
        plane_id: "local".into(),
        plane: PlaneId::Local,
        listener,
        clock: Arc::new(ManualTimer::default()),
        client_id,
    }
}

impl FakePlane {
    pub(crate) fn bridge(&self) -> &JetBridge {
        &self.setup.bridge
    }

    /// The next connection, handshaken.
    pub(crate) async fn accept(&self) -> (Reader, Writer) {
        accept(&self.listener, self.client_id).await
    }

    /// Moves the remote Plane's deadline clock.
    pub(crate) fn advance(&self, by: Duration) {
        self.clock.advance(by);
    }

    /// The next connection, whose handshake the Plane refuses with `error`
    /// (for example after `jetd` was replaced by an incompatible build).
    pub(crate) async fn reject_handshake(&self, error: WireError) {
        use tokio::io::AsyncReadExt;

        let (mut stream, _) = self.listener.accept().await.unwrap();
        let mut preface = vec![0; jet_protocol::PREFACE.len()];
        stream.read_exact(&mut preface).await.unwrap();
        let (read, write) = stream.into_split();
        let mut reader = FrameReader::new(read);
        let mut writer = FrameWriter::new(write);
        let Frame::Control { .. } = reader.read().await.unwrap() else {
            panic!("expected a control-frame hello");
        };
        let rejected = encode_control(&ServerHello::Rejected { error }).unwrap();
        writer.write(&Frame::control(rejected)).await.unwrap();
    }
}

/// Runs `request` against `serve`, returning as soon as the request ends,
/// with what `serve` returned if it had finished. A request that fails
/// before reaching the Plane ends the exchange instead of leaving the
/// script waiting for a connection that never comes.
pub(crate) async fn exchange<T, U>(
    request: impl std::future::Future<Output = T>,
    serve: impl std::future::Future<Output = U>,
) -> (T, Option<U>) {
    tokio::pin!(request);
    tokio::pin!(serve);
    let mut served = None;
    loop {
        tokio::select! {
            result = &mut request => return (result, served),
            value = &mut serve, if served.is_none() => served = Some(value),
        }
    }
}

pub(crate) async fn next(reader: &mut Reader) -> (StreamId, ClientMessage) {
    next_message(reader).await
}

/// The Command ID of a Command message.
pub(crate) fn command_id(message: &ClientMessage) -> Uuid {
    match message {
        ClientMessage::Command { command_id, .. } => *command_id,
        other => panic!("expected a Command, got {other:?}"),
    }
}

pub(crate) async fn answer(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    result: QueryResponse,
) {
    reply(
        writer,
        stream,
        ServerMessage::QueryResult {
            id: request_id(message),
            result,
        },
    )
    .await;
}

pub(crate) async fn complete(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    result: CommandResponse,
) {
    reply(
        writer,
        stream,
        ServerMessage::CommandResult {
            id: request_id(message),
            result,
        },
    )
    .await;
}

pub(crate) async fn refuse(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    error: WireError,
) {
    reply(
        writer,
        stream,
        ServerMessage::Error {
            id: Some(request_id(message)),
            error,
        },
    )
    .await;
}

/// A stable daemon refusal without detail.
pub(crate) fn wire(category: ErrorCategory, code: &str) -> WireError {
    WireError {
        category,
        code: code.into(),
        retryable: false,
        message: "daemon text never crosses".into(),
        revision_conflict: None,
        restart: None,
        recovery_actions: Vec::new(),
    }
}
