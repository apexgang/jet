//! Reaching a remote Plane: the system ssh, then Jet's signed remote login
//! (`Client::connect_remote`) with a key unlocked before ssh starts.
//!
//! There is no other transport. `RemoteConnector` can only be built from an
//! `SshEndpoint`, and its only I/O is the `SshSpawner`; a failure is
//! classified and returned, never retried over anything else.
//!
//! Design note (session caching): local Planes connect per request, but a
//! remote session is cached, because spawning ssh plus a signed handshake
//! for every request is not viable. A cached session cannot outlive a
//! daemon restart: jetd closes the relay, `jetd connect` exits, ssh exits,
//! and the next `connect()` sees the exited child and logs in again. Request
//! paths that see `Closed` also `invalidate` the session.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use jet_client::{Client, ClientError, SshEndpoint};
use jet_protocol::{
    ErrorCategory, FrameError, PlaneStatus, RemotePairingRequest, RemotePairingResponse,
};
use uuid::Uuid;

use super::{
    spawner::{SshChild, SshSpawner},
    unknown_plane, ConnectionView, Observed, PlaneId, DUPLICATES_LOCAL,
};
use crate::jet::{
    deadline::{Deadlines, Wait},
    errors::PublicError,
    keystore::{Credential, IdentityKeys},
    pairing_transcript::identity_changed,
};

/// Retryable failures wait this long before the next ssh spawn.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];
/// How long a failed handshake waits for ssh's exit status.
const EXIT_WAIT: Duration = Duration::from_millis(500);

/// One authenticated remote session. Dropping it kills ssh.
pub(crate) struct RemoteSession {
    client: Arc<Client>,
    child: Box<dyn SshChild>,
}

impl RemoteSession {
    pub(crate) fn client(&self) -> Arc<Client> {
        Arc::clone(&self.client)
    }
}

struct Failure {
    error: PublicError,
    /// `None` is sticky: only Retry, Pair again or Forget clears it.
    next_attempt: Option<Instant>,
}

#[derive(Default)]
struct Slot {
    session: Option<RemoteSession>,
    failure: Option<Failure>,
    backoff: usize,
    /// A session was live before, so a failure is a reconnect.
    was_online: bool,
}

/// Single-flight connection owner for one remote Plane.
pub(crate) struct RemoteConnector {
    plane: PlaneId,
    endpoint: SshEndpoint,
    client_id: Uuid,
    expected_identity: Uuid,
    credential: Mutex<Credential>,
    spawner: Arc<dyn SshSpawner>,
    keys: Arc<IdentityKeys>,
    observed: Arc<Mutex<Observed>>,
    slot: tokio::sync::Mutex<Slot>,
    /// Set when the Plane is forgotten: no login happens after that, so a
    /// request that was in flight cannot reach the Plane again.
    closed: AtomicBool,
    deadlines: Deadlines,
}

impl std::fmt::Debug for RemoteConnector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteConnector")
            .field("plane", &self.plane)
            .finish_non_exhaustive()
    }
}

pub(crate) struct ConnectorParts {
    pub(crate) plane: PlaneId,
    pub(crate) endpoint: SshEndpoint,
    pub(crate) client_id: Uuid,
    pub(crate) expected_identity: Uuid,
    pub(crate) credential: Credential,
    pub(crate) spawner: Arc<dyn SshSpawner>,
    pub(crate) keys: Arc<IdentityKeys>,
    pub(crate) observed: Arc<Mutex<Observed>>,
    pub(crate) deadlines: Deadlines,
}

impl RemoteConnector {
    /// The only constructor: an SSH endpoint and the Plane identity it must
    /// prove on every login. `session` adopts a login the caller just made.
    pub(crate) fn new(parts: ConnectorParts, session: Option<RemoteSession>) -> Self {
        let was_online = session.is_some();
        Self {
            plane: parts.plane,
            endpoint: parts.endpoint,
            client_id: parts.client_id,
            expected_identity: parts.expected_identity,
            credential: Mutex::new(parts.credential),
            spawner: parts.spawner,
            keys: parts.keys,
            observed: parts.observed,
            slot: tokio::sync::Mutex::new(Slot {
                session,
                was_online,
                ..Slot::default()
            }),
            closed: AtomicBool::new(false),
            deadlines: parts.deadlines,
        }
    }

    pub(crate) fn deadlines(&self) -> &Deadlines {
        &self.deadlines
    }

    fn show(&self, connection: ConnectionView) {
        if let Ok(mut observed) = self.observed.lock() {
            observed.connection = connection;
        }
    }

    pub(crate) fn credential(&self) -> Credential {
        self.credential
            .lock()
            .map(|credential| *credential)
            .unwrap_or(Credential::Durable)
    }

    /// A pairing replaced this computer's key on the Plane (repair in place).
    pub(crate) fn set_credential(&self, credential: Credential) {
        if let Ok(mut current) = self.credential.lock() {
            *current = credential;
        }
    }

    /// The cached session, or one fresh login. Parallel callers share one
    /// attempt and never spawn parallel ssh processes; a stored failure is
    /// returned without spawning while it is sticky or backing off.
    pub(crate) async fn connect(&self) -> Result<Arc<Client>, PublicError> {
        if self.is_closed() {
            return Err(self.closed_error());
        }
        let mut slot = self.slot.lock().await;
        if self.is_closed() {
            slot.session = None;
            return Err(self.closed_error());
        }
        if let Some(session) = slot.session.as_mut() {
            if !session.child.has_exited() {
                return Ok(session.client());
            }
            slot.session = None;
        }
        if let Some(failure) = &slot.failure {
            match failure.next_attempt {
                None => return Err(failure.error.clone()),
                Some(at) if Instant::now() < at => return Err(failure.error.clone()),
                Some(_) => {}
            }
        }
        match slot.failure.as_ref() {
            Some(failure) if slot.was_online => self.show(ConnectionView::Reconnecting {
                error: failure.error.clone(),
            }),
            _ => self.show(ConnectionView::Connecting),
        }
        let credential = self.credential();
        match login(
            self.spawner.as_ref(),
            &self.keys,
            &self.endpoint,
            self.client_id,
            credential,
            Some(self.expected_identity),
            &self.deadlines,
        )
        .await
        {
            Ok((session, status)) => {
                if self.is_closed() {
                    // Forgotten while logging in: dropping the session kills ssh.
                    drop(session);
                    return Err(self.closed_error());
                }
                if let Ok(mut observed) = self.observed.lock() {
                    observed.logged_in(&status);
                }
                let client = session.client();
                slot.session = Some(session);
                slot.failure = None;
                slot.backoff = 0;
                slot.was_online = true;
                Ok(client)
            }
            Err(error) => {
                let error = error.with_plane(self.plane.to_string());
                self.record(&mut slot, error.clone());
                Err(error)
            }
        }
    }

    fn record(&self, slot: &mut Slot, error: PublicError) {
        let next_attempt = error.retryable.then(|| {
            let delay = BACKOFF[slot.backoff.min(BACKOFF.len() - 1)];
            slot.backoff = slot.backoff.saturating_add(1);
            Instant::now() + delay
        });
        self.show(if next_attempt.is_some() {
            ConnectionView::Reconnecting {
                error: error.clone(),
            }
        } else {
            ConnectionView::Failed {
                error: error.clone(),
            }
        });
        slot.failure = Some(Failure {
            error,
            next_attempt,
        });
    }

    /// The user's Retry: clears a sticky or backed-off failure. A remote
    /// entry that is this computer's own Plane stays failed: only Forget
    /// ends it, whatever the webview asks.
    pub(crate) async fn reset(&self) {
        let mut slot = self.slot.lock().await;
        if slot
            .failure
            .as_ref()
            .is_some_and(|failure| failure.error.code == DUPLICATES_LOCAL)
        {
            return;
        }
        slot.failure = None;
        slot.backoff = 0;
    }

    /// Drops the cached session if it is still `client` (ssh is killed).
    pub(crate) async fn invalidate(&self, client: &Arc<Client>) {
        let mut slot = self.slot.lock().await;
        if slot
            .session
            .as_ref()
            .is_some_and(|session| Arc::ptr_eq(&session.client, client))
        {
            slot.session = None;
        }
    }

    /// A sticky failure the shell concluded itself (lost access after a
    /// self-change, a duplicate of the local Plane). The session is dropped.
    pub(crate) async fn fail(&self, error: PublicError) {
        let mut slot = self.slot.lock().await;
        slot.session = None;
        let error = error.with_plane(self.plane.to_string());
        self.show(ConnectionView::Failed {
            error: error.clone(),
        });
        slot.failure = Some(Failure {
            error,
            next_attempt: None,
        });
    }

    /// Records a sticky failure without waiting, for example a session-only
    /// pairing after a restart. Returns false while a login holds the slot.
    pub(crate) fn fail_now(&self, error: PublicError) -> bool {
        let Ok(mut slot) = self.slot.try_lock() else {
            return false;
        };
        {
            let error = error.with_plane(self.plane.to_string());
            self.show(ConnectionView::Failed {
                error: error.clone(),
            });
            slot.session = None;
            slot.failure = Some(Failure {
                error,
                next_attempt: None,
            });
        }
        true
    }

    /// Kills ssh; a later request logs in again. Used by Pair again.
    pub(crate) async fn shutdown(&self) {
        self.slot.lock().await.session = None;
    }

    /// The Plane was forgotten: kills ssh and refuses every later login. It
    /// never waits for a login in flight (which may sit in a keyring
    /// prompt); that login drops its own session when it ends.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.slot.try_lock() {
            slot.session = None;
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn closed_error(&self) -> PublicError {
        unknown_plane().with_plane(self.plane.to_string())
    }

    /// How long until the connector would try again, so feed loops pace
    /// themselves instead of polling a stored failure.
    pub(crate) fn retry_in(&self) -> Option<Duration> {
        let slot = self.slot.try_lock().ok()?;
        slot.failure
            .as_ref()
            .and_then(|failure| failure.next_attempt)
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    #[cfg(test)]
    pub(crate) async fn has_session(&self) -> bool {
        self.slot.lock().await.session.is_some()
    }
}

/// Unlocks the identity, spawns ssh and runs the signed remote login. With
/// `expected`, a different `status.plane_id` is `plane.identity_changed`.
/// `jet-client` bounds the signed handshake itself (15 s); the status read
/// that follows waits at most the liveness deadline, since a link that
/// stops answering would otherwise hold the connector's single-flight slot
/// forever. The keyring unlock before ssh has its own prompt limit.
pub(crate) async fn login(
    spawner: &dyn SshSpawner,
    keys: &IdentityKeys,
    endpoint: &SshEndpoint,
    client_id: Uuid,
    credential: Credential,
    expected: Option<Uuid>,
    deadlines: &Deadlines,
) -> Result<(RemoteSession, PlaneStatus), PublicError> {
    // Before ssh starts: a keyring prompt inside the handshake would always
    // lose the server's 10-second nonce window.
    let identity = keys.unlock_for_handshake(client_id, credential).await?;
    let spawned = spawner
        .spawn(endpoint)
        .map_err(|error| classify(None, Some(error.kind())))?;
    let mut child = spawned.child;
    let connected = Client::connect_remote(spawned.read, spawned.write, &identity).await;
    drop(identity);
    let client = match connected {
        Ok(client) => Arc::new(client),
        Err(error) => return Err(handshake_failure(&error, child.as_mut()).await),
    };
    let status = match deadlines.bound(Wait::Liveness, client.status()).await {
        Ok(status) => status,
        Err(error) => return Err(request_failure(&error, child.as_mut()).await),
    };
    if expected.is_some_and(|expected| expected != status.plane_id) {
        return Err(identity_changed());
    }
    Ok((RemoteSession { client, child }, status))
}

/// A restricted pairing exchange over a fresh ssh process, which is killed
/// as soon as the reply arrives.
pub(crate) async fn enroll(
    spawner: &dyn SshSpawner,
    endpoint: &SshEndpoint,
    client_id: Uuid,
    request: &RemotePairingRequest,
) -> Result<RemotePairingResponse, Box<EnrollFailure>> {
    let spawned = spawner.spawn(endpoint).map_err(|error| {
        Box::new(EnrollFailure {
            error: classify(None, Some(error.kind())),
            definite: true,
        })
    })?;
    let mut child = spawned.child;
    match Client::pair_remote(spawned.read, spawned.write, client_id, request).await {
        Ok(response) => Ok(response),
        Err(error) => {
            // Only a stable Plane refusal is a durable answer; everything
            // else may have reached the Plane and keeps its command ID.
            let definite = matches!(
                &error,
                ClientError::Remote(wire) | ClientError::Rejected(wire)
                    if wire.category != ErrorCategory::OutcomeUnknown
            );
            Err(Box::new(EnrollFailure {
                error: handshake_failure(&error, child.as_mut()).await,
                definite,
            }))
        }
    }
}

pub(crate) struct EnrollFailure {
    pub(crate) error: PublicError,
    pub(crate) definite: bool,
}

fn transport_lost(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io(_)
            | ClientError::Closed
            | ClientError::Frame(FrameError::Io(_) | FrameError::Closed)
    )
}

fn timed_out(error: &ClientError) -> bool {
    matches!(error, ClientError::Io(io) if io.kind() == std::io::ErrorKind::TimedOut)
}

/// Maps a failure of `connect_remote` or `pair_remote`. `Unexpected` from
/// these call sites means the endpoint skipped or downgraded authentication.
async fn handshake_failure(error: &ClientError, child: &mut dyn SshChild) -> PublicError {
    if let ClientError::Unexpected(_) = error {
        return handshake_refused();
    }
    request_failure(error, child).await
}

async fn request_failure(error: &ClientError, child: &mut dyn SshChild) -> PublicError {
    if timed_out(error) {
        return PublicError::offline();
    }
    if transport_lost(error) {
        let exit = child.wait_exit(EXIT_WAIT).await;
        return classify(exit, None);
    }
    PublicError::from_client(error)
}

fn handshake_refused() -> PublicError {
    PublicError::invalid_response_code(
        "plane.handshake_refused",
        "The Plane answered in a way Jet doesn't trust, so Jet stopped.",
    )
}

/// ssh's exit status, or why it could not start. Stderr is never read.
pub(crate) fn classify(exit: Option<i32>, spawn_error: Option<std::io::ErrorKind>) -> PublicError {
    if let Some(kind) = spawn_error {
        return if kind == std::io::ErrorKind::NotFound {
            PublicError::unavailable(
                "ssh.client_missing",
                "OpenSSH isn't installed on this computer.",
                false,
            )
        } else {
            PublicError::offline()
        };
    }
    match exit {
        // Host-key, authentication and network failures all exit 255.
        Some(255) => PublicError::unavailable(
            "ssh.connection_failed",
            "Jet couldn't connect over SSH. Check that `ssh <address>` works in a terminal.",
            true,
        ),
        // The remote shell could not find `jetd` (often a shorter PATH).
        Some(127) => PublicError::unavailable(
            "plane.jetd_missing",
            "Jet isn't installed on that computer, or jetd isn't on the PATH SSH uses.",
            false,
        ),
        // `jetd connect` exits 1 when the daemon socket is unreachable.
        Some(1) => PublicError::unavailable(
            "plane.jetd_unavailable",
            "Jet isn't running on that computer.",
            true,
        ),
        _ => PublicError::offline(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use jet_protocol::{
        decode_control, encode_control, ClientHello, ClientMessage, ConnectionProof, Frame,
        FrameReader, FrameWriter, QueryRequest, QueryResponse, RecoveryState, RecoveryStatus,
        ServerHello, ServerMessage, StreamId, WireError, REMOTE_AUTH_MINOR,
    };
    use tokio::io::{AsyncReadExt, DuplexStream, ReadHalf, WriteHalf};

    use super::*;
    use crate::jet::{
        deadline::{manual::ManualTimer, LIVENESS_DEADLINE, QUERY_DEADLINE},
        keystore::{
            tests::{CountingStore, Fail},
            IdentityKeys,
        },
        planes::spawner::fake::FakeSpawner,
    };

    pub(crate) type ServerReader = FrameReader<ReadHalf<DuplexStream>>;
    pub(crate) type ServerWriter = FrameWriter<WriteHalf<DuplexStream>>;

    pub(crate) const CLIENT: Uuid = Uuid::from_u128(7);
    pub(crate) const PLANE: Uuid = Uuid::from_u128(0xabc);

    /// What the far end of ssh received as its first control message.
    pub(crate) enum Opening {
        Login {
            reader: ServerReader,
            writer: ServerWriter,
            hello: ClientHello,
            nonce: [u8; 32],
            proof: ConnectionProof,
        },
        Pairing {
            writer: ServerWriter,
            request: RemotePairingRequest,
        },
    }

    /// Reads the preface and hello, sends a challenge and returns what the
    /// client answered with.
    pub(crate) async fn open(stream: DuplexStream) -> Opening {
        let (mut read, write) = tokio::io::split(stream);
        let mut preface = vec![0; jet_protocol::PREFACE.len()];
        read.read_exact(&mut preface).await.unwrap();
        assert_eq!(preface, jet_protocol::PREFACE);
        let mut reader = FrameReader::new(read);
        let mut writer = FrameWriter::new(write);
        let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
            panic!("expected hello");
        };
        let hello: ClientHello = decode_control(&payload).unwrap();
        let nonce = [0x5a; 32];
        writer
            .write(&Frame::control(
                encode_control(&ServerHello::Challenge { nonce }).unwrap(),
            ))
            .await
            .unwrap();
        let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
            panic!("expected proof or pairing request");
        };
        match decode_control::<ConnectionProof>(&payload) {
            Ok(proof) => Opening::Login {
                reader,
                writer,
                hello,
                nonce,
                proof,
            },
            Err(_) => Opening::Pairing {
                writer,
                request: decode_control(&payload).unwrap(),
            },
        }
    }

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

    pub(crate) async fn answer_pairing(writer: &mut ServerWriter, response: RemotePairingResponse) {
        writer
            .write(&Frame::control(encode_control(&response).unwrap()))
            .await
            .unwrap();
    }

    pub(crate) async fn reject(writer: &mut ServerWriter, error: WireError) {
        writer
            .write(&Frame::control(
                encode_control(&ServerHello::Rejected { error }).unwrap(),
            ))
            .await
            .unwrap();
    }

    /// Verifies the Ed25519 proof, then sends Welcome.
    pub(crate) async fn welcome(opening: Opening, key: &[u8; 32]) -> (ServerReader, ServerWriter) {
        let Opening::Login {
            mut reader,
            mut writer,
            hello,
            nonce,
            proof,
        } = opening
        else {
            panic!("expected a login");
        };
        assert_eq!(hello.client_id, CLIENT);
        let transcript = jet_protocol::connection_signing_bytes(&hello, &nonce).unwrap();
        VerifyingKey::from_bytes(key)
            .unwrap()
            .verify(&transcript, &Signature::from_bytes(&proof.signature))
            .expect("the proof verifies with this computer's public key");
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

    pub(crate) async fn next_message(
        reader: &mut ServerReader,
    ) -> Option<(StreamId, ClientMessage)> {
        match reader.read().await {
            Ok(Frame::Control { stream_id, payload }) => {
                Some((stream_id, decode_control(&payload).unwrap()))
            }
            _ => None,
        }
    }

    pub(crate) async fn reply(writer: &mut ServerWriter, stream: StreamId, message: ServerMessage) {
        writer
            .write(&Frame::stream_control(
                stream,
                encode_control(&message).unwrap(),
            ))
            .await
            .unwrap();
    }

    pub(crate) fn request_id(message: &ClientMessage) -> u64 {
        match message {
            ClientMessage::Query { id, .. } | ClientMessage::Command { id, .. } => *id,
            other => panic!("unexpected request: {other:?}"),
        }
    }

    pub(crate) fn status(identity: Uuid) -> PlaneStatus {
        PlaneStatus {
            cursor: Some(3),
            plane_id: identity,
            daemon_starts: 1,
            started_at_unix_ms: 1,
            core_version: "0.3.0".into(),
            security: None,
            recovery: Some(RecoveryStatus {
                state: RecoveryState::Serving,
                reason: None,
                snapshots: Vec::new(),
                deletion_ledger: None,
            }),
        }
    }

    /// Answers status queries with `identity` until the client leaves.
    pub(crate) async fn serve_status(reader: ServerReader, writer: ServerWriter, identity: Uuid) {
        serve_status_with(reader, writer, status(identity)).await;
    }

    /// Answers status queries with `answer` until the client leaves.
    pub(crate) async fn serve_status_with(
        mut reader: ServerReader,
        mut writer: ServerWriter,
        answer: PlaneStatus,
    ) {
        while let Some((stream, message)) = next_message(&mut reader).await {
            match &message {
                ClientMessage::Query {
                    query: QueryRequest::Status,
                    ..
                } => {
                    let id = request_id(&message);
                    reply(
                        &mut writer,
                        stream,
                        ServerMessage::QueryResult {
                            id,
                            result: QueryResponse::Status(answer.clone()),
                        },
                    )
                    .await;
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
    }

    pub(crate) struct Harness {
        pub(crate) spawner: Arc<FakeSpawner>,
        pub(crate) store: Arc<CountingStore>,
        pub(crate) observed: Arc<Mutex<Observed>>,
        pub(crate) connector: Arc<RemoteConnector>,
        pub(crate) key: [u8; 32],
    }

    pub(crate) async fn harness(fail: Fail) -> Harness {
        harness_on(fail, Deadlines::default()).await
    }

    pub(crate) async fn harness_on(fail: Fail, deadlines: Deadlines) -> Harness {
        let spawner = Arc::new(FakeSpawner::default());
        let store = Arc::new(CountingStore::new(Fail::Nothing));
        let keys = Arc::new(IdentityKeys::new(store.clone()));
        let key = keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        *store.fail.lock().unwrap() = fail;
        let observed = Arc::new(Mutex::new(Observed::default()));
        let connector = Arc::new(RemoteConnector::new(
            ConnectorParts {
                plane: PlaneId::Remote(Uuid::from_u128(2)),
                endpoint: SshEndpoint::new("alice@build-box").unwrap(),
                client_id: CLIENT,
                expected_identity: PLANE,
                credential: Credential::Durable,
                spawner: spawner.clone(),
                keys: keys.clone(),
                observed: observed.clone(),
                deadlines,
            },
            None,
        ));
        Harness {
            spawner,
            store,
            observed,
            connector,
            key,
        }
    }

    #[test]
    fn exit_statuses_map_to_stable_codes() {
        let code = |exit, spawn| classify(exit, spawn).code;
        assert_eq!(code(Some(255), None), "ssh.connection_failed");
        assert_eq!(code(Some(127), None), "plane.jetd_missing");
        assert_eq!(code(Some(1), None), "plane.jetd_unavailable");
        assert_eq!(code(Some(2), None), "transport.offline");
        assert_eq!(code(None, None), "transport.offline");
        assert_eq!(
            code(None, Some(std::io::ErrorKind::NotFound)),
            "ssh.client_missing"
        );
        assert_eq!(
            code(None, Some(std::io::ErrorKind::PermissionDenied)),
            "transport.offline"
        );
        assert!(classify(Some(255), None).retryable);
        assert!(!classify(Some(127), None).retryable);
        assert!(classify(Some(1), None).retryable);
        assert!(!classify(None, Some(std::io::ErrorKind::NotFound)).retryable);
        // No classification ever carries a recovery action the UI would
        // follow with another transport: only Retry, Pair again or Open
        // Planes, which the UI derives from the code.
        for exit in [Some(255), Some(127), Some(1), Some(0), None] {
            assert!(classify(exit, None).recovery_actions.is_empty());
        }
    }

    #[tokio::test]
    async fn an_endpoint_that_skips_authentication_is_refused_by_call_site() {
        let error = handshake_failure(
            &ClientError::Unexpected("remote endpoint skipped authentication".into()),
            &mut NoChild,
        )
        .await;
        assert_eq!(error.code, "plane.handshake_refused");
        assert_eq!(error.category, "invalid_response");
        assert!(!error.retryable);
        let timeout = handshake_failure(
            &ClientError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "t")),
            &mut NoChild,
        )
        .await;
        assert_eq!(timeout.code, "transport.offline");
        let refused = handshake_failure(
            &ClientError::Rejected(wire(ErrorCategory::Unauthorized, "connection.unauthorized")),
            &mut NoChild,
        )
        .await;
        assert_eq!(refused.code, "connection.unauthorized");
        assert_eq!(refused.category, "unauthorized");
        assert!(!refused.retryable);
    }

    struct NoChild;
    impl SshChild for NoChild {
        fn has_exited(&mut self) -> bool {
            true
        }
        fn wait_exit(&mut self, _: Duration) -> crate::jet::planes::spawner::ExitFuture<'_> {
            Box::pin(async { Some(255) })
        }
    }

    #[tokio::test]
    async fn a_login_proves_the_key_and_seeds_knowledge_from_status() {
        let harness = harness(Fail::Nothing).await;
        let key = harness.key;
        harness.spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            serve_status(reader, writer, PLANE).await;
            Some(0)
        });
        harness.connector.connect().await.unwrap();
        {
            let observed = harness.observed.lock().unwrap();
            assert_eq!(observed.connection, ConnectionView::Online);
            assert_eq!(observed.identity, Some(PLANE));
            let protocol = serde_json::to_value(observed.knowledge.view()).unwrap();
            assert!(protocol["atLeast"].as_u64().unwrap() >= 37);
        }

        // A Plane whose status carries no recovery field still proved the
        // remote-auth minor by accepting the signed login.
        let older_plane = self::harness(Fail::Nothing).await;
        let key = older_plane.key;
        older_plane.spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            let mut older = status(PLANE);
            older.recovery = None;
            serve_status_with(reader, writer, older).await;
            Some(0)
        });
        older_plane.connector.connect().await.unwrap();
        let observed = older_plane.observed.lock().unwrap();
        let protocol = serde_json::to_value(observed.knowledge.view()).unwrap();
        assert!(protocol["atLeast"].as_u64().unwrap() >= u64::from(REMOTE_AUTH_MINOR));
    }

    /// D6: the Plane accepts the signed login, then never answers its
    /// status read. The login ends at the liveness deadline as a retryable
    /// failure with backoff, releasing the single-flight slot instead of
    /// holding it (and every request to this Plane) forever. `jet-client`
    /// bounds the handshake itself.
    #[tokio::test]
    async fn a_login_whose_status_never_arrives_backs_off_at_the_deadline() {
        let clock = Arc::new(ManualTimer::default());
        let harness = harness_on(Fail::Nothing, Deadlines::on(clock.clone())).await;
        let key = harness.key;
        let (asked, status_seen) = tokio::sync::oneshot::channel();
        harness.spawner.push(move |stream| async move {
            let (mut reader, writer) = welcome(open(stream).await, &key).await;
            let status = next_message(&mut reader).await;
            let _ = asked.send(status.is_some());
            // Hold the link open and never answer.
            let _link = (reader, writer);
            std::future::pending::<Option<i32>>().await
        });
        let (outcome, ()) = tokio::join!(harness.connector.connect(), async {
            assert!(status_seen.await.unwrap());
            clock.advance(LIVENESS_DEADLINE);
        });
        let error = outcome.err().unwrap();
        assert_eq!(error.code, "transport.offline");
        assert!(error.retryable);
        assert!(!harness.connector.has_session().await);
        assert!(matches!(
            harness.observed.lock().unwrap().connection,
            ConnectionView::Reconnecting { .. }
        ));
        // The failure is stored with backoff: nothing is spawned again yet.
        assert_eq!(
            harness.connector.connect().await.err().unwrap().code,
            "transport.offline"
        );
        assert_eq!(harness.spawner.spawns(), 1);
    }

    /// D6: a request on a cached session that stops answering ends at its
    /// deadline, and the session is dropped so the next request logs in
    /// again rather than waiting on the same dead link.
    #[tokio::test]
    async fn a_hung_session_is_dropped_at_the_request_deadline() {
        let clock = Arc::new(ManualTimer::default());
        let harness = harness_on(Fail::Nothing, Deadlines::on(clock.clone())).await;
        let key = harness.key;
        let (asked, question_seen) = tokio::sync::oneshot::channel();
        harness.spawner.push(move |stream| async move {
            let (mut reader, mut writer) = welcome(open(stream).await, &key).await;
            let (stream, message) = next_message(&mut reader).await.unwrap();
            let id = request_id(&message);
            reply(
                &mut writer,
                stream,
                ServerMessage::QueryResult {
                    id,
                    result: QueryResponse::Status(status(PLANE)),
                },
            )
            .await;
            let question = next_message(&mut reader).await;
            let _ = asked.send(question.is_some());
            let _link = (reader, writer);
            std::future::pending::<Option<i32>>().await
        });
        let client = crate::jet::client::PlaneClient::remote(harness.connector.clone(), CLIENT);
        let connection = client.connect().await.unwrap();
        assert!(harness.connector.has_session().await);
        let (outcome, ()) = tokio::join!(connection.query(connection.projects()), async {
            assert!(question_seen.await.unwrap());
            clock.advance(QUERY_DEADLINE);
        });
        let error = PublicError::from_client(&outcome.unwrap_err());
        assert_eq!((error.category, error.retryable), ("offline", true));
        assert!(!harness.connector.has_session().await);
    }

    #[tokio::test]
    async fn a_closed_connector_never_logs_in_again() {
        let harness = harness(Fail::Nothing).await;
        let key = harness.key;
        harness.spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            serve_status(reader, writer, PLANE).await;
            Some(0)
        });
        harness.connector.connect().await.unwrap();
        harness.connector.close();
        assert!(!harness.connector.has_session().await);
        let error = harness.connector.connect().await.err().unwrap();
        assert_eq!(error.code, "plane.unknown");
        harness.connector.reset().await;
        assert!(harness.connector.connect().await.is_err());
        assert_eq!(harness.spawner.spawns(), 1);
    }

    #[tokio::test]
    async fn retry_keeps_a_duplicate_of_the_local_plane_failed() {
        let harness = harness(Fail::Nothing).await;
        harness
            .connector
            .fail(super::super::duplicates_local())
            .await;
        harness.connector.reset().await;
        let error = harness.connector.connect().await.err().unwrap();
        assert_eq!(error.code, DUPLICATES_LOCAL);
        assert_eq!(harness.spawner.spawns(), 0);
    }

    #[tokio::test]
    async fn a_different_plane_behind_the_same_address_is_refused() {
        let harness = harness(Fail::Nothing).await;
        let key = harness.key;
        harness.spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            serve_status(reader, writer, Uuid::from_u128(0xdef)).await;
            Some(0)
        });
        let error = harness.connector.connect().await.err().unwrap();
        assert_eq!(error.code, "plane.identity_changed");
        assert!(!harness.connector.has_session().await);
        // Sticky: nothing is spawned again until the user acts.
        assert_eq!(
            harness.connector.connect().await.err().unwrap().code,
            "plane.identity_changed"
        );
        assert_eq!(harness.spawner.spawns(), 1);
    }

    #[tokio::test]
    async fn parallel_connects_share_one_ssh_process() {
        let harness = harness(Fail::Nothing).await;
        let key = harness.key;
        harness.spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            serve_status(reader, writer, PLANE).await;
            Some(0)
        });
        let attempts = (0..10).map(|_| {
            let connector = harness.connector.clone();
            tokio::spawn(async move { connector.connect().await })
        });
        let clients: Vec<_> = futures_join(attempts).await;
        assert!(clients.iter().all(Result::is_ok));
        let first = clients[0].as_ref().unwrap();
        assert!(clients
            .iter()
            .all(|client| Arc::ptr_eq(client.as_ref().unwrap(), first)));
        assert_eq!(harness.spawner.spawns(), 1);
    }

    async fn futures_join(
        handles: impl Iterator<Item = tokio::task::JoinHandle<Result<Arc<Client>, PublicError>>>,
    ) -> Vec<Result<Arc<Client>, PublicError>> {
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap());
        }
        results
    }

    #[tokio::test]
    async fn an_unauthorized_login_is_sticky_until_reset() {
        let harness = harness(Fail::Nothing).await;
        let served = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let served = served.clone();
            harness.spawner.push(move |stream| async move {
                let Opening::Login { mut writer, .. } = open(stream).await else {
                    panic!("expected a login");
                };
                reject(
                    &mut writer,
                    wire(ErrorCategory::Unauthorized, "connection.unauthorized"),
                )
                .await;
                served.fetch_add(1, Ordering::SeqCst);
                Some(0)
            });
        }
        for _ in 0..3 {
            let error = harness.connector.connect().await.err().unwrap();
            assert_eq!(error.code, "connection.unauthorized");
            assert_eq!(
                error.plane_id.as_deref(),
                Some(&*Uuid::from_u128(2).to_string())
            );
        }
        assert_eq!(harness.spawner.spawns(), 1);
        assert!(matches!(
            harness.observed.lock().unwrap().connection,
            ConnectionView::Failed { .. }
        ));
        harness.connector.reset().await;
        assert!(harness.connector.connect().await.is_err());
        assert_eq!(harness.spawner.spawns(), 2);
        assert_eq!(served.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_retryable_failure_spawns_again_only_after_its_backoff() {
        let harness = harness(Fail::Nothing).await;
        harness.spawner.push(|stream| async move {
            drop(stream);
            Some(255)
        });
        let error = harness.connector.connect().await.err().unwrap();
        assert_eq!(error.code, "ssh.connection_failed");
        assert!(matches!(
            harness.observed.lock().unwrap().connection,
            ConnectionView::Reconnecting { .. }
        ));
        // Before next_attempt_at the stored error returns without a spawn.
        assert_eq!(
            harness.connector.connect().await.err().unwrap().code,
            "ssh.connection_failed"
        );
        assert_eq!(harness.spawner.spawns(), 1);
        assert!(harness.connector.retry_in().unwrap() <= BACKOFF[0]);
        // Once the backoff passes, one new attempt is made.
        {
            let mut slot = harness.connector.slot.lock().await;
            slot.failure.as_mut().unwrap().next_attempt = Some(Instant::now());
        }
        harness.spawner.push(|stream| async move {
            drop(stream);
            Some(1)
        });
        assert_eq!(
            harness.connector.connect().await.err().unwrap().code,
            "plane.jetd_unavailable"
        );
        assert_eq!(harness.spawner.spawns(), 2);
        assert_eq!(harness.connector.slot.lock().await.backoff, 2);
    }

    #[tokio::test]
    async fn a_locked_keyring_spawns_nothing() {
        let harness = harness(Fail::Locked).await;
        let error = harness.connector.connect().await.err().unwrap();
        assert_eq!(error.code, "identity.secret_store_locked");
        assert_eq!(harness.spawner.spawns(), 0);
        assert!(harness.store.count() > 0);
    }

    #[tokio::test]
    async fn a_missing_ssh_client_and_an_exited_relay_are_classified() {
        let harness = harness(Fail::Nothing).await;
        // No script queued: the fake reports the program as not found.
        let error = harness.connector.connect().await.err().unwrap();
        assert_eq!(error.code, "ssh.client_missing");
        assert!(!error.retryable);

        let harness = self::harness(Fail::Nothing).await;
        let key = harness.key;
        harness.spawner.push(move |stream| async move {
            let (mut reader, mut writer) = welcome(open(stream).await, &key).await;
            // Answer the login status, then exit like a restarted daemon.
            let (stream, message) = next_message(&mut reader).await.unwrap();
            let id = request_id(&message);
            reply(
                &mut writer,
                stream,
                ServerMessage::QueryResult {
                    id,
                    result: QueryResponse::Status(status(PLANE)),
                },
            )
            .await;
            Some(1)
        });
        let first = harness.connector.connect().await.unwrap();
        // Wait until the fake relay has exited.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        harness.spawner.push(move |stream| async move {
            let (reader, writer) = welcome(open(stream).await, &key).await;
            serve_status(reader, writer, PLANE).await;
            Some(0)
        });
        let second = harness.connector.connect().await.unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "an exited relay is not reused"
        );
        assert_eq!(harness.spawner.spawns(), 2);
        harness.connector.invalidate(&first).await;
        assert!(
            harness.connector.has_session().await,
            "only the same session is dropped"
        );
        harness.connector.invalidate(&second).await;
        assert!(!harness.connector.has_session().await);
    }
}
