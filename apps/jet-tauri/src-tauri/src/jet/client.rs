use std::{
    future::Future,
    ops::Deref,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use jet_client::{Client, ClientError};
use jet_protocol::{
    AccountBinding, CapabilityObservation, CredentialSource, Event, PlaneStatus, Presentation,
    PresentationBlock, Project, ProjectDisposal, ProjectRemovalBinding, ProjectRemoved,
};
use uuid::Uuid;

use jet_protocol::{SettingKey, SettingScope};
use serde::Deserialize;

use super::{
    deadline::{self, Deadlines, Wait},
    errors::{PublicError, ToPublic},
    planes::remote::RemoteConnector,
};

/// How a Plane is reached. `Remote` wraps only a `RemoteConnector`, whose
/// sole constructor takes an `SshEndpoint`: there is no other transport to
/// fall back to.
#[derive(Clone)]
enum Transport {
    Local { socket: Arc<PathBuf> },
    Remote(Arc<RemoteConnector>),
}

/// A failed attempt to reach a Plane or to complete a request on it.
#[derive(Debug)]
pub(crate) enum ConnectError {
    Client(ClientError),
    /// Classified by the shell while reaching a remote Plane.
    Plane(PublicError),
    /// A reconnect that failed after an earlier attempt of the same request
    /// may have reached the Plane (`PlaneClient::with_reconnect`). Only the
    /// Plane's own answer could settle that attempt, so this never does,
    /// whatever the failure itself says (a refused handshake, an older
    /// protocol).
    Unconfirmed(Box<ConnectError>),
}

impl ConnectError {
    /// Whether this failure settles a Command (`command_ids::definite`).
    /// A shell-classified connect failure sent nothing it could settle.
    pub(crate) fn definite(&self) -> bool {
        match self {
            Self::Client(error) => super::command_ids::definite(error),
            Self::Plane(_) | Self::Unconfirmed(_) => false,
        }
    }
}

impl ToPublic for ConnectError {
    fn to_public(&self) -> PublicError {
        match self {
            Self::Client(error) => PublicError::from_client(error),
            Self::Plane(error) => error.clone(),
            Self::Unconfirmed(error) => error.to_public(),
        }
    }
}

impl From<ClientError> for Box<ConnectError> {
    fn from(error: ClientError) -> Self {
        Box::new(ConnectError::Client(error))
    }
}

impl From<Box<ClientError>> for Box<ConnectError> {
    fn from(error: Box<ClientError>) -> Self {
        Box::new(ConnectError::Client(*error))
    }
}

#[derive(Clone)]
pub(crate) struct PlaneClient {
    transport: Transport,
    client_id: Uuid,
    reconnect_delays: Arc<[Duration]>,
    event_poll_delay: Duration,
    /// How long a request may wait for this Plane (`deadline.rs`).
    deadlines: Deadlines,
}

impl PlaneClient {
    pub(crate) fn new(
        socket: PathBuf,
        client_id: Uuid,
        reconnect_delays: impl Into<Arc<[Duration]>>,
        event_poll_delay: Duration,
    ) -> Self {
        Self {
            transport: Transport::Local {
                socket: Arc::new(socket),
            },
            client_id,
            reconnect_delays: reconnect_delays.into(),
            event_poll_delay,
            deadlines: Deadlines::default(),
        }
    }

    /// A remote Plane: one cached session, one retry after a stale session,
    /// and a slower event poll than the local socket. It keeps the
    /// connector's deadlines.
    pub(crate) fn remote(connector: Arc<RemoteConnector>, client_id: Uuid) -> Self {
        Self {
            deadlines: connector.deadlines().clone(),
            transport: Transport::Remote(connector),
            client_id,
            reconnect_delays: Arc::new([Duration::from_secs(1)]),
            event_poll_delay: Duration::from_secs(1),
        }
    }

    /// The same Plane with another deadline clock (tests).
    #[cfg(test)]
    pub(crate) fn with_deadlines(mut self, deadlines: Deadlines) -> Self {
        self.deadlines = deadlines;
        self
    }

    pub(crate) fn deadlines(&self) -> &Deadlines {
        &self.deadlines
    }

    pub(crate) fn client_id(&self) -> Uuid {
        self.client_id
    }

    pub(crate) fn connector(&self) -> Option<&Arc<RemoteConnector>> {
        match &self.transport {
            Transport::Local { .. } => None,
            Transport::Remote(connector) => Some(connector),
        }
    }

    #[cfg(test)]
    pub(crate) fn socket_for_test(&self) -> &str {
        match &self.transport {
            Transport::Local { socket } => socket.to_str().unwrap_or_default(),
            Transport::Remote(_) => "",
        }
    }

    /// Runs `call` on a connection, reconnecting after a lost transport with
    /// the same arguments (so a mutation keeps its command ID). A stale
    /// remote session is invalidated before the reconnect. Each attempt is
    /// bounded by `wait`'s deadline, and an expired one is not retried: the
    /// Plane is hung, not gone, and a Command's outcome is now unknown.
    ///
    /// Once an attempt ran and lost its transport, the request may have
    /// reached the Plane, so a later failure that is not the Plane's answer
    /// to it (a refused handshake from a replaced `jetd`, an older protocol)
    /// is returned as `ConnectError::Unconfirmed`: the caller keeps the ID.
    async fn with_reconnect<T, F, Fut>(
        &self,
        wait: Wait,
        mut call: F,
    ) -> Result<T, Box<ConnectError>>
    where
        F: FnMut(Arc<Client>) -> Fut,
        Fut: Future<Output = Result<T, ClientError>>,
    {
        let mut attempt = 0;
        let mut maybe_sent = false;
        loop {
            let (client, error) = match self.connect_client().await {
                Ok(client) => match self.deadlines.bound(wait, call(Arc::clone(&client))).await {
                    Ok(value) => return Ok(value),
                    Err(error) => (Some(client), Box::new(ConnectError::Client(*error))),
                },
                Err(error) => (None, error),
            };
            let retry = reconnectable(&error);
            if retry {
                if let Some(client) = &client {
                    self.invalidate_client(client).await;
                }
            }
            if !retry || expired(&error) || attempt >= self.reconnect_delays.len() {
                let answered = matches!(*error, ConnectError::Client(ClientError::Remote(_)));
                return Err(if maybe_sent && !answered {
                    Box::new(ConnectError::Unconfirmed(error))
                } else {
                    error
                });
            }
            maybe_sent |= client.is_some();
            self.wait_to_reconnect(attempt).await;
            attempt += 1;
        }
    }

    pub(crate) async fn status(&self) -> Result<PlaneStatus, Box<ConnectError>> {
        self.with_reconnect(
            Wait::Liveness,
            |client| async move { client.status().await },
        )
        .await
    }

    pub(crate) async fn register_project(
        &self,
        command_id: Uuid,
        path: &str,
    ) -> Result<Project, Box<ConnectError>> {
        self.with_reconnect(Wait::Command, |client| {
            let path = path.to_owned();
            async move { client.register_project(command_id, &path).await }
        })
        .await
    }

    pub(crate) async fn bind_harness_account(
        &self,
        command_id: Uuid,
        provider: &str,
        label: &str,
    ) -> Result<AccountBinding, Box<ConnectError>> {
        self.with_reconnect(Wait::Command, |client| {
            let (provider, label) = (provider.to_owned(), label.to_owned());
            async move {
                client
                    .bind_account(
                        command_id,
                        &provider,
                        &label,
                        None,
                        CredentialSource::HarnessNative,
                    )
                    .await
            }
        })
        .await
    }

    pub(crate) async fn remove_project(
        &self,
        command_id: Uuid,
        binding: ProjectRemovalBinding,
        typed_name: &str,
        disposal: ProjectDisposal,
    ) -> Result<ProjectRemoved, Box<ConnectError>> {
        self.with_reconnect(Wait::Command, |client| {
            let (binding, disposal) = (binding.clone(), disposal.clone());
            let typed_name = typed_name.to_owned();
            async move {
                client
                    .remove_project(command_id, binding, &typed_name, disposal)
                    .await
            }
        })
        .await
    }

    pub(crate) async fn current_capabilities(
        &self,
    ) -> Result<jet_protocol::CapabilitySnapshot, Box<ConnectError>> {
        let client = self.connect().await?;
        Ok(client
            .query(client.capabilities(CapabilityObservation::Fresh))
            .await?)
    }

    #[cfg(test)]
    pub(crate) async fn clear_setting_with_retry(
        &self,
        command_id: Uuid,
        key: SettingKey,
        scope: SettingScope,
    ) -> Result<(), Box<ConnectError>> {
        self.with_reconnect(Wait::Command, |client| async move {
            client.clear_setting(command_id, key, scope).await
        })
        .await
    }

    pub(crate) async fn stream_updates<F>(&self, mut cursor: u64, mut send: F)
    where
        F: FnMut(NativeUpdate) -> bool,
    {
        let mut reconnect_attempt = 0usize;
        loop {
            let client = match self.connect_client().await {
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
            let mut replay_through = 0;
            loop {
                // A page that never arrives is a lost transport: the feed
                // reports Reconnecting and dials again (`deadline.rs`).
                match self
                    .deadlines
                    .bound(Wait::Liveness, client.events_after(cursor))
                    .await
                {
                    Ok(page) => {
                        if !resumed {
                            replay_through = page.cursor;
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
                            let mut summary = EventSummary::from_event(event);
                            if summary.sequence <= replay_through {
                                summary.notification = None;
                            }
                            if !send(NativeUpdate::Event(summary)) {
                                return;
                            }
                        }
                        if cursor >= page.cursor {
                            tokio::time::sleep(self.event_poll_delay).await;
                        }
                    }
                    Err(error) => {
                        let public = PublicError::from_client(&error);
                        if !reconnectable_client(&error) {
                            let _ = send(NativeUpdate::Failed { error: public });
                            return;
                        }
                        self.invalidate_client(&client).await;
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

    /// A handshaken connection whose requests carry this Plane's deadlines.
    /// Local Planes connect per request, within the liveness deadline (a
    /// stopped `jetd` still accepts on its socket); a remote Plane returns
    /// its cached session or makes one single-flight login, whose handshake
    /// has its own deadline (`remote::login`).
    pub(crate) async fn connect(&self) -> Result<Connection, Box<ConnectError>> {
        Ok(Connection {
            client: self.connect_client().await?,
            plane: self.clone(),
            expired: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn connect_client(&self) -> Result<Arc<Client>, Box<ConnectError>> {
        match &self.transport {
            Transport::Local { socket } => self
                .deadlines
                .bound(
                    Wait::Liveness,
                    Client::connect_local(socket.as_ref(), self.client_id),
                )
                .await
                .map(Arc::new)
                .map_err(|error| Box::new(ConnectError::Client(*error))),
            Transport::Remote(connector) => connector
                .connect()
                .await
                .map_err(|error| Box::new(ConnectError::Plane(error))),
        }
    }

    /// Drops a remote session that failed underneath a request. Local
    /// connections are per request, so there is nothing to drop.
    pub(crate) async fn invalidate(&self, connection: &Connection) {
        self.invalidate_client(&connection.client).await;
    }

    async fn invalidate_client(&self, client: &Arc<Client>) {
        if let Transport::Remote(connector) = &self.transport {
            connector.invalidate(client).await;
        }
    }

    /// The user's Retry: clears a remote Plane's sticky or backed-off
    /// failure. The local Plane has no such state.
    pub(crate) async fn reset(&self) {
        if let Transport::Remote(connector) = &self.transport {
            connector.reset().await;
        }
    }

    /// Waits before the next reconnect. A remote Plane also waits out its
    /// connector's backoff, so a feed never polls a stored failure.
    pub(crate) async fn wait_to_reconnect(&self, attempt: usize) {
        let mut delay = self
            .reconnect_delays
            .get(attempt)
            .or_else(|| self.reconnect_delays.last())
            .copied()
            .unwrap_or(Duration::from_millis(100));
        if let Transport::Remote(connector) = &self.transport {
            delay = delay.max(connector.retry_in().unwrap_or_default());
        }
        tokio::time::sleep(delay).await;
    }

    /// Pacing for loops that retry `status()` themselves: remote Planes wait
    /// for their backoff; the local status helper already waits internally.
    pub(crate) async fn pace(&self) {
        if matches!(self.transport, Transport::Remote(_)) {
            self.wait_to_reconnect(0).await;
        }
    }
}

/// Whether a request or connect outlived its deadline.
fn expired(error: &ConnectError) -> bool {
    matches!(error, ConnectError::Client(error) if deadline::expired_wait(error).is_some())
}

/// A handshaken connection to one Plane. It dereferences to the protocol
/// client; `query` and `command` bound one request by the Plane's deadlines
/// (`deadline.rs`), which every Plane request of the shell goes through.
#[derive(Clone)]
pub(crate) struct Connection {
    client: Arc<Client>,
    plane: PlaneClient,
    /// Set once a request here outlived its deadline: the peer is hung.
    expired: Arc<AtomicBool>,
}

impl Deref for Connection {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}

impl Connection {
    /// Awaits one Query (or a read-only transfer) for at most the Query
    /// deadline. An expiry is a lost transport: offline and retryable.
    pub(crate) async fn query<T>(
        &self,
        request: impl Future<Output = Result<T, ClientError>>,
    ) -> Result<T, Box<ClientError>> {
        self.bounded(Wait::Query, request).await
    }

    /// Awaits one Command for at most the Command deadline. An expiry
    /// leaves the outcome unknown (`command.outcome_unknown`): callers keep
    /// the Command ID, as for any lost transport, so a retry resends it.
    pub(crate) async fn command<T>(
        &self,
        request: impl Future<Output = Result<T, ClientError>>,
    ) -> Result<T, Box<ClientError>> {
        self.bounded(Wait::Command, request).await
    }

    /// Once one request outlives its deadline the peer is hung, local or
    /// remote alike: every later request on this connection fails at once
    /// as offline (`Closed`, nothing is sent) instead of waiting a full
    /// deadline each, which a flow of many reads would add up. A remote
    /// session that stopped answering is also dropped (ssh is killed), so
    /// the next connection logs in again instead of reusing the dead link.
    async fn bounded<T>(
        &self,
        wait: Wait,
        request: impl Future<Output = Result<T, ClientError>>,
    ) -> Result<T, Box<ClientError>> {
        if self.expired.load(Ordering::Acquire) {
            return Err(Box::new(ClientError::Closed));
        }
        let result = self.plane.deadlines.bound(wait, request).await;
        if let Err(error) = &result {
            if deadline::expired_wait(error).is_some() {
                self.expired.store(true, Ordering::Release);
                self.plane.invalidate_client(&self.client).await;
            }
        }
        result
    }
}

fn reconnectable(error: &ConnectError) -> bool {
    match error {
        ConnectError::Client(error) => reconnectable_client(error),
        // The connector already applied its own retry and backoff policy.
        ConnectError::Plane(_) => false,
        ConnectError::Unconfirmed(_) => false,
    }
}

fn reconnectable_client(error: &ClientError) -> bool {
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
    pub(crate) notification: Option<super::notifications::NotificationSignal>,
    pub(crate) sequence: u64,
    pub(crate) recorded_at_unix_ms: i64,
    pub(crate) kind: String,
    pub(crate) conversation_id: Option<Uuid>,
    pub(crate) run_id: Option<Uuid>,
    pub(crate) timeline: Vec<TimelineProjection>,
    /// Which Setting a `setting.changed` or `setting.cleared` event names.
    /// Only the Settings change watcher reads it; the main feed never does.
    pub(crate) setting: Option<SettingEventProjection>,
}

/// The identifiers of a Setting event. The value never crosses: the
/// Settings window reloads the section instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettingEventProjection {
    pub(crate) key: &'static str,
    pub(crate) scope: &'static str,
    pub(crate) project_id: Option<Uuid>,
}

pub(crate) struct TimelineProjection {
    pub(crate) kind: &'static str,
    pub(crate) text: String,
    pub(crate) item_id: Option<String>,
    pub(crate) approval: Option<ApprovalProjection>,
}

pub(crate) struct ApprovalProjection {
    pub(crate) request_id: String,
    pub(crate) review_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) tool: String,
    pub(crate) action: String,
    pub(crate) target: String,
    pub(crate) scope: &'static str,
    pub(crate) consequence: String,
    pub(crate) rationale: Option<String>,
    pub(crate) state: &'static str,
    pub(crate) can_authorize_retry: bool,
}

impl EventSummary {
    pub(crate) fn from_event(event: Event) -> Self {
        let timeline = timeline_projection(&event);
        let notification = super::notifications::NotificationSignal::from_event(&event);
        let setting = setting_projection(&event);
        Self {
            notification,
            sequence: event.sequence,
            recorded_at_unix_ms: event.recorded_at_unix_ms,
            kind: safe_event_kind(&event.kind),
            conversation_id: event.conversation_id,
            run_id: event.run_id,
            timeline,
            setting,
        }
    }
}

/// ASVS 1.5.2: typed, allowlisted identifiers only. The payload is parsed
/// into `SettingKey`/`SettingScope`, extra fields (including the value) are
/// ignored, and anything else projects to nothing.
fn setting_projection(event: &Event) -> Option<SettingEventProjection> {
    #[derive(serde::Deserialize)]
    struct Named {
        key: SettingKey,
        scope: SettingScope,
    }
    if !matches!(event.kind.as_str(), "setting.changed" | "setting.cleared") {
        return None;
    }
    let named = Named::deserialize(&event.payload).ok()?;
    let (scope, project_id) = match named.scope {
        SettingScope::Plane => ("plane", None),
        SettingScope::Project { project_id } => ("project", Some(project_id)),
        SettingScope::Conversation { .. } => ("conversation", None),
    };
    Some(SettingEventProjection {
        key: super::settings::key_spelling(named.key),
        scope,
        project_id,
    })
}

fn timeline_projection(event: &Event) -> Vec<TimelineProjection> {
    match event.kind.as_str() {
        "turn.input" => {
            let Some(turn_id) = event
                .payload
                .get("turn_id")
                .and_then(|value| value.as_str())
            else {
                return Vec::new();
            };
            let Some(text) = event.payload.get("text").and_then(|value| value.as_str()) else {
                return Vec::new();
            };
            vec![TimelineProjection {
                kind: "user",
                text: bounded_event_text(text, 8_192),
                item_id: Uuid::parse_str(turn_id).ok().map(|id| id.to_string()),
                approval: None,
            }]
        }
        "run.output" => output_projection(&event.payload),
        "run.activity_changed" => vec![TimelineProjection {
            kind: "activity",
            text: activity_text(event.payload.get("activity")),
            item_id: None,
            approval: None,
        }],
        "run.lifecycle_changed" => {
            let to = event
                .payload
                .get("to")
                .and_then(|value| value.as_str())
                .unwrap_or("updated");
            vec![TimelineProjection {
                kind: "activity",
                text: format!("Run {}.", to.replace('_', " ")),
                item_id: None,
                approval: None,
            }]
        }
        "run.control_requested" => vec![TimelineProjection {
            kind: "activity",
            text: control_request_text(event.payload.get("control")),
            item_id: None,
            approval: None,
        }],
        "run.terminated" => vec![TimelineProjection {
            kind: "result",
            text: termination_text(&event.payload),
            item_id: None,
            approval: None,
        }],
        "approval.requested" | "approval.reviewed" => approval_projection(event)
            .into_iter()
            .map(|approval| TimelineProjection {
                kind: "approval",
                text: approval_summary(&approval),
                item_id: Some(format!(
                    "approval-{}-{}",
                    approval.run_id.as_deref().unwrap_or("run"),
                    approval.request_id
                )),
                approval: Some(approval),
            })
            .collect(),
        "approval.retry_authorized" => vec![TimelineProjection {
            kind: "result",
            text: "One exact approval retry was authorized.".into(),
            item_id: None,
            approval: None,
        }],
        "change.checkpoint_recorded" => vec![TimelineProjection {
            kind: "result",
            text: "Jet recorded the completed turn and its changes.".into(),
            item_id: None,
            approval: None,
        }],
        _ => Vec::new(),
    }
}

fn output_projection(payload: &serde_json::Value) -> Vec<TimelineProjection> {
    let mut projected = Vec::new();
    let Some(blocks) = payload
        .get("presentation_json")
        .and_then(|value| value.as_array())
    else {
        return projected;
    };
    for raw in blocks.iter().take(32).filter_map(|value| value.as_str()) {
        let Ok(block) = serde_json::from_str::<PresentationBlock>(raw) else {
            continue;
        };
        match block.known() {
            Ok(Some(Presentation::Text { text } | Presentation::Markdown { text })) => {
                projected.push(TimelineProjection {
                    kind: "agent",
                    text: bounded_event_text(&text, 16_384),
                    item_id: None,
                    approval: None,
                });
            }
            Ok(Some(Presentation::Actions { actions })) => {
                let count = actions.len().min(128);
                projected.push(TimelineProjection {
                    kind: "activity",
                    text: format!(
                        "{count} Run action{} available.",
                        if count == 1 { " is" } else { "s are" }
                    ),
                    item_id: None,
                    approval: None,
                });
            }
            Ok(None) => projected.push(TimelineProjection {
                kind: "activity",
                text: "The Run published an additional presentation block.".into(),
                item_id: None,
                approval: None,
            }),
            Err(_) => {}
        }
    }
    projected
}

fn approval_projection(event: &Event) -> Option<ApprovalProjection> {
    let (review_id, request, state, can_authorize_retry, rationale, consequence) = if event.kind
        == "approval.requested"
    {
        (
                None,
                event.payload.get("request")?,
                "requested",
                false,
                None,
                "The Run stays paused until this request is decided. Manual approval decisions are not available through the current client protocol.".to_owned(),
            )
    } else {
        let review = event.payload.get("review")?;
        let outcome = review.get("outcome")?;
        let status = outcome.get("status")?.as_str()?;
        let decision = outcome.get("decision").and_then(|value| value.as_str());
        let state = match (status, decision) {
            ("decided", Some("allow")) => "allowed",
            ("denied", _) | ("decided", Some("deny")) => "denied",
            ("unavailable", _) => "unavailable",
            _ => return None,
        };
        let can_retry = state == "denied";
        let rationale = outcome
            .get("verdict")
            .and_then(|value| value.get("rationale"))
            .and_then(|value| value.as_str())
            .or_else(|| outcome.get("reason").and_then(|value| value.as_str()))
            .map(|value| bounded_event_text(value, 1_024));
        let consequence = match state {
                "allowed" => "The reviewer allowed this exact action once.",
                "denied" => {
                    "The action remains blocked. You may authorize one review retry of the unchanged request."
                }
                _ => {
                    "Automatic review could not decide. The Run remains paused for a person."
                }
            }
            .to_owned();
        (
            review
                .get("review_id")
                .and_then(|value| value.as_str())
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(|value| value.to_string()),
            review.get("request")?,
            state,
            can_retry,
            rationale,
            consequence,
        )
    };
    let request_id = request.get("request_id")?.as_str()?;
    let tool = request.get("tool")?.as_str()?;
    let action = request.get("action")?.as_str()?;
    if request_id.is_empty()
        || request_id.len() > 128
        || request_id.chars().any(char::is_control)
        || tool.is_empty()
        || tool.len() > 128
        || tool.chars().any(char::is_control)
        || action.len() > 4_096
    {
        return None;
    }
    // ASVS 1.2.1 and 1.5.2: approval content stays bounded data. The
    // webview receives text fields only and Svelte performs contextual escaping.
    Some(ApprovalProjection {
        request_id: request_id.to_owned(),
        review_id,
        run_id: event.run_id.map(|id| id.to_string()),
        tool: tool.to_owned(),
        action: action.to_owned(),
        target: action_target(action),
        scope: "This action once",
        consequence,
        rationale,
        state,
        can_authorize_retry,
    })
}

fn action_target(action: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(action) else {
        return "Current Run".into();
    };
    let Some(object) = value.as_object() else {
        return "Current Run".into();
    };
    for key in [
        "file_path",
        "filePath",
        "path",
        "directory",
        "cwd",
        "working_directory",
        "workingDirectory",
    ] {
        if let Some(value) = object.get(key).and_then(|value| value.as_str()) {
            return one_line(value, 512, "Current Run");
        }
    }
    "Current Run".into()
}

fn one_line(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    let flattened = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.is_empty() {
        fallback.into()
    } else {
        bounded_event_text(&flattened, maximum_bytes)
    }
}

fn approval_summary(approval: &ApprovalProjection) -> String {
    match approval.state {
        "allowed" => format!("{} was allowed once.", approval.tool),
        "denied" => format!("{} was denied.", approval.tool),
        "unavailable" => format!("{} still needs a decision.", approval.tool),
        _ => format!("{} needs approval.", approval.tool),
    }
}

fn control_request_text(value: Option<&serde_json::Value>) -> String {
    match value.and_then(|value| value.as_str()) {
        Some("interrupt_turn") => "Interrupt requested. Waiting for the active Turn to end.".into(),
        Some("stop_run") => "Stop requested. Waiting for the Run to end.".into(),
        _ => "Run control requested.".into(),
    }
}

fn termination_text(payload: &serde_json::Value) -> String {
    let termination = payload.get("termination").unwrap_or(payload);
    let control = termination.get("control").and_then(|value| value.as_str());
    let stage = termination.get("stage").and_then(|value| value.as_str());
    match (control, stage) {
        (Some("interrupt_turn"), Some("native_cancellation")) => {
            "The active Turn was interrupted. The Run can accept the next Turn.".into()
        }
        (Some("interrupt_turn"), Some(_)) => {
            "Interrupting the Turn required ending the Run process.".into()
        }
        (Some("stop_run"), Some("unobserved")) => {
            "Jet sent every stop signal but could not observe the Run ending.".into()
        }
        (Some("stop_run"), Some(_)) => "The Run stopped and kept its recorded work.".into(),
        _ => "The Run control request finished.".into(),
    }
}

fn activity_text(value: Option<&serde_json::Value>) -> String {
    let label = value
        .and_then(|value| value.get("activity").or(Some(value)))
        .and_then(|value| value.as_str())
        .unwrap_or("idle")
        .replace('_', " ");
    if label == "idle" {
        "Run activity paused.".into()
    } else {
        format!("Run is {label}.")
    }
}

fn bounded_event_text(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.into();
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[Output truncated by the desktop client.]",
        &value[..end]
    )
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
pub(crate) mod unit_tests {
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

    use super::{safe_event_kind, EventSummary, NativeUpdate, PlaneClient};
    use crate::jet::{
        deadline::{manual::ManualTimer, Deadlines, LIVENESS_DEADLINE, QUERY_DEADLINE},
        errors::PublicError,
    };

    fn on_manual_clock(socket: std::path::PathBuf) -> (PlaneClient, std::sync::Arc<ManualTimer>) {
        let clock = std::sync::Arc::new(ManualTimer::default());
        let client = PlaneClient::new(
            socket,
            Uuid::from_u128(7),
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        )
        .with_deadlines(Deadlines::on(clock.clone()));
        (client, clock)
    }

    /// D6: a stopped `jetd` still accepts on its socket but never finishes
    /// the handshake. The status read ends at the liveness deadline as a
    /// retryable offline failure, and is not retried behind the caller.
    #[tokio::test]
    async fn a_stopped_local_jetd_is_offline_at_the_liveness_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("jetd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (client, clock) = on_manual_clock(socket);

        let (status, stopped) = tokio::join!(client.status(), async {
            let (stream, _) = listener.accept().await.unwrap();
            clock.advance(LIVENESS_DEADLINE);
            stream
        });
        let error = PublicError::from_client(&status.unwrap_err());
        assert_eq!(
            (error.category, error.code.as_str(), error.retryable),
            ("offline", "transport.offline", true)
        );
        drop(stopped);
        let mut next = Box::pin(listener.accept());
        assert!(
            crate::jet::deadline::tests::still_pending(&mut next).await,
            "an expired wait is not retried"
        );
    }

    /// D6: once a request on a connection outlives its deadline, the peer is
    /// hung. A local `jetd` keeps its socket open, so without this each
    /// later read of a many-read flow (Trash names, Autodelete lookups)
    /// would wait a full deadline of its own; instead it fails at once as
    /// offline, as it does on a remote Plane whose session was dropped.
    #[tokio::test]
    async fn a_connection_that_expired_fails_later_requests_at_once() {
        use std::{future::Future, task::Poll};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("jetd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (client, clock) = on_manual_clock(socket);
        let client_id = client.client_id();
        let (connection, (mut reader, _writer)) =
            tokio::join!(client.connect(), accept(&listener, client_id));
        let connection = connection.unwrap();

        let (first, ()) = tokio::join!(connection.query(connection.status()), async {
            let _ = next_message(&mut reader).await;
            clock.advance(QUERY_DEADLINE);
        });
        let first = PublicError::from_client(&first.unwrap_err());
        assert_eq!(first.code, "transport.offline");

        let second = connection.query(connection.status());
        tokio::pin!(second);
        let Poll::Ready(second) =
            std::future::poll_fn(|context| Poll::Ready(second.as_mut().poll(context))).await
        else {
            panic!("the second read waited for a deadline of its own");
        };
        let second = PublicError::from_client(&second.unwrap_err());
        assert_eq!(
            (second.code.as_str(), second.retryable),
            ("transport.offline", true)
        );
    }

    /// D6: a feed whose page never arrives reports Reconnecting at the
    /// liveness deadline and dials again, instead of staying "Connected".
    #[tokio::test]
    async fn a_hung_feed_reports_reconnecting_and_dials_again() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("jetd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (client, clock) = on_manual_clock(socket);
        let client_id = client.client_id();

        let mut updates = Vec::new();
        let ((), hung) = tokio::join!(
            client.stream_updates(10, |update| {
                let resumed_again = matches!(update, NativeUpdate::Resumed { .. })
                    && updates.contains(&"reconnecting");
                updates.push(match update {
                    NativeUpdate::Resumed { .. } => "resumed",
                    NativeUpdate::Event(_) => "event",
                    NativeUpdate::Reconnecting { error } => {
                        assert_eq!(error.code, "transport.offline");
                        assert!(error.retryable);
                        "reconnecting"
                    }
                    NativeUpdate::Failed { .. } => "failed",
                });
                !resumed_again
            }),
            async {
                let (mut reader, writer) = accept(&listener, client_id).await;
                let (_, page) = next_message(&mut reader).await;
                assert_events_after(&page, 10);
                // The Plane stops answering: no page, no close.
                clock.advance(LIVENESS_DEADLINE);
                let (mut again, mut writer_again) = accept(&listener, client_id).await;
                let (stream, page) = next_message(&mut again).await;
                assert_events_after(&page, 10);
                let id = request_id(&page);
                reply(
                    &mut writer_again,
                    stream,
                    ServerMessage::QueryResult {
                        id,
                        result: QueryResponse::Events(EventPage {
                            cursor: 10,
                            events: Vec::new(),
                        }),
                    },
                )
                .await;
                (reader, writer, again, writer_again)
            }
        );
        drop(hung);
        assert_eq!(updates, ["reconnecting", "resumed"]);
    }

    #[test]
    fn event_kind_is_bounded_before_it_reaches_the_webview() {
        assert_eq!(
            safe_event_kind("run.lifecycle_changed"),
            "run.lifecycle_changed"
        );
        assert_eq!(safe_event_kind("<img src=x onerror=alert(1)>"), "unknown");
    }

    #[test]
    fn timeline_projects_only_portable_inert_presentation() {
        let turn_id = Uuid::from_u128(9);
        let input = EventSummary::from_event(Event {
            sequence: 7,
            event_id: Uuid::from_u128(7),
            actor: Actor::InteractiveClient {
                client_id: Uuid::from_u128(1),
            },
            origin: None,
            recorded_at_unix_ms: 1,
            conversation_id: Some(Uuid::from_u128(2)),
            run_id: Some(Uuid::from_u128(3)),
            kind: "turn.input".into(),
            payload_version: 1,
            payload: serde_json::json!({
                "turn_id": turn_id,
                "text": "Ship Wave 1.3",
                "private": "not forwarded"
            }),
        });
        assert_eq!(input.timeline.len(), 1);
        assert_eq!(input.timeline[0].kind, "user");
        assert_eq!(input.timeline[0].text, "Ship Wave 1.3");
        assert_eq!(
            input.timeline[0].item_id.as_deref(),
            Some(turn_id.to_string().as_str())
        );

        let output = EventSummary::from_event(Event {
            sequence: 8,
            event_id: Uuid::from_u128(8),
            actor: Actor::InteractiveClient {
                client_id: Uuid::from_u128(1),
            },
            origin: None,
            recorded_at_unix_ms: 2,
            conversation_id: Some(Uuid::from_u128(2)),
            run_id: Some(Uuid::from_u128(3)),
            kind: "run.output".into(),
            payload_version: 1,
            payload: serde_json::json!({
                "native_json": r#"{"secret":true}"#,
                "presentation_json": [
                    r#"{"kind":"markdown","text":"**Done**"}"#,
                    r#"{"kind":"future","private":"hidden"}"#
                ]
            }),
        });
        assert_eq!(output.timeline.len(), 2);
        assert_eq!(output.timeline[0].kind, "agent");
        assert_eq!(output.timeline[0].text, "**Done**");
        assert_eq!(output.timeline[1].kind, "activity");
        assert!(output
            .timeline
            .iter()
            .all(|item| !item.text.contains("secret")));
        assert!(output
            .timeline
            .iter()
            .all(|item| !item.text.contains("private")));

        let lifecycle = EventSummary::from_event(Event {
            sequence: 9,
            event_id: Uuid::from_u128(9),
            actor: Actor::InteractiveClient {
                client_id: Uuid::from_u128(1),
            },
            origin: None,
            recorded_at_unix_ms: 3,
            conversation_id: Some(Uuid::from_u128(2)),
            run_id: Some(Uuid::from_u128(3)),
            kind: "run.lifecycle_changed".into(),
            payload_version: 1,
            payload: serde_json::json!({ "to": "active" }),
        });
        assert_eq!(lifecycle.timeline.len(), 1);
        assert_eq!(lifecycle.timeline[0].text, "Run active.");
        assert_eq!(lifecycle.timeline[0].item_id, None);
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

    pub(crate) async fn accept(
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

    pub(crate) async fn next_message(
        reader: &mut FrameReader<OwnedReadHalf>,
    ) -> (StreamId, ClientMessage) {
        let Frame::Control { stream_id, payload } = reader.read().await.unwrap() else {
            panic!("expected a control-frame request");
        };
        (stream_id, decode_control(&payload).unwrap())
    }

    pub(crate) async fn reply(
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

    pub(crate) fn request_id(message: &ClientMessage) -> u64 {
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

    pub(crate) fn status(cursor: u64) -> PlaneStatus {
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
