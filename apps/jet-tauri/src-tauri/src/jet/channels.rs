use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use jet_protocol::PlaneStatus;
use serde::Serialize;
use tauri::{ipc::Channel, State};
use tokio::task::AbortHandle;
use uuid::Uuid;

use super::{
    client::{ApprovalProjection, EventSummary, NativeUpdate, PlaneClient},
    errors::PublicError,
    notifications::NotificationState,
    planes::{ConnectionView, HealthView, PlaneHealth, PlaneId, PlaneRegistry},
    JetBridge,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionSnapshot {
    state: &'static str,
    feed_id: String,
    plane_id: String,
    plane_identity: Option<String>,
    health: HealthView,
    core_version: Option<String>,
    daemon_starts: Option<String>,
    started_at_unix_ms: Option<String>,
    cursor: Option<String>,
}

impl ConnectionSnapshot {
    fn from_status(feed_id: Uuid, plane: PlaneId, status: PlaneStatus) -> Self {
        Self {
            state: "online",
            feed_id: feed_id.to_string(),
            plane_id: plane.to_string(),
            plane_identity: Some(status.plane_id.to_string()),
            health: PlaneHealth::from_status(&status).view(),
            core_version: Some(bounded_text(&status.core_version, 48, "Unknown")),
            daemon_starts: Some(status.daemon_starts.to_string()),
            started_at_unix_ms: Some(status.started_at_unix_ms.to_string()),
            cursor: Some(status.cursor.unwrap_or_default().to_string()),
        }
    }

    fn reconnecting(feed_id: Uuid, plane: PlaneId, registry: &PlaneRegistry) -> Self {
        Self {
            state: "reconnecting",
            feed_id: feed_id.to_string(),
            plane_id: plane.to_string(),
            plane_identity: registry.identity(plane).map(|id| id.to_string()),
            health: registry.health(plane).view(),
            core_version: None,
            daemon_starts: None,
            started_at_unix_ms: None,
            cursor: None,
        }
    }
}

/// One live feed task per Plane. Opening a feed aborts that Plane's previous
/// task, so recoveries and reconnects never leave an orphaned poll loop.
#[derive(Default)]
pub(crate) struct FeedRegistry {
    by_plane: Mutex<HashMap<PlaneId, (Uuid, AbortHandle)>>,
}

impl FeedRegistry {
    /// Makes `feed` the current feed of `plane` and aborts the one it replaces.
    fn replace(&self, plane: PlaneId, feed: Uuid, task: AbortHandle) {
        match self.by_plane.lock() {
            Ok(mut feeds) => {
                if let Some((_, previous)) = feeds.insert(plane, (feed, task)) {
                    previous.abort();
                }
            }
            // Without the registry the task could not be stopped later.
            Err(_) => task.abort(),
        }
    }

    /// Aborts the feed only while it is still the current one.
    pub(crate) fn close(&self, feed: Uuid) -> Result<(), PublicError> {
        let mut feeds = self.by_plane.lock().map_err(|_| PublicError::internal())?;
        let plane = feeds
            .iter()
            .find_map(|(plane, (current, _))| (*current == feed).then_some(*plane));
        if let Some((_, task)) = plane.and_then(|plane| feeds.remove(&plane)) {
            task.abort();
        }
        Ok(())
    }

    /// Aborts whatever feed a forgotten Plane still has.
    pub(crate) fn forget(&self, plane: PlaneId) {
        if let Ok(mut feeds) = self.by_plane.lock() {
            if let Some((_, task)) = feeds.remove(&plane) {
                task.abort();
            }
        }
    }

    /// Forgets a task that ended by itself, without touching a newer feed.
    fn finished(&self, plane: PlaneId, feed: Uuid) {
        if let Ok(mut feeds) = self.by_plane.lock() {
            if feeds
                .get(&plane)
                .is_some_and(|(current, _)| *current == feed)
            {
                feeds.remove(&plane);
            }
        }
    }
}

/// Everything a feed task needs, cloned out of managed state.
struct FeedContext {
    app: tauri::AppHandle,
    plane: PlaneId,
    feed: Uuid,
    client: PlaneClient,
    planes: Arc<PlaneRegistry>,
    feeds: Arc<FeedRegistry>,
    notifications: Arc<NotificationState>,
}

impl FeedContext {
    fn registration(&self) -> (Arc<FeedRegistry>, PlaneId, Uuid) {
        (Arc::clone(&self.feeds), self.plane, self.feed)
    }

    /// The task ended by itself (channel closed or terminal failure).
    fn finish(&self) {
        self.feeds.finished(self.plane, self.feed);
    }

    /// A successful status read: seed Plane knowledge and fence notifications
    /// so connecting never replays the Plane's history.
    fn connected(&self, status: &PlaneStatus) {
        self.planes.observe_status(self.plane, status);
        self.planes
            .set_connection(self.plane, ConnectionView::Online);
        self.notifications
            .fence(self.plane, status.cursor.unwrap_or_default());
    }

    async fn stream(&self, cursor: u64, on_update: &Channel<PlaneUpdate>) {
        let label = self.planes.notification_label(self.plane);
        self.client
            .stream_updates(cursor, |update| {
                match &update {
                    NativeUpdate::Event(event) => self.notifications.observe(
                        &self.app,
                        self.plane,
                        label.as_deref(),
                        event.sequence,
                        event.notification,
                    ),
                    NativeUpdate::Resumed { .. } => self
                        .planes
                        .set_connection(self.plane, ConnectionView::Online),
                    NativeUpdate::Reconnecting { error } => self.planes.set_connection(
                        self.plane,
                        ConnectionView::Reconnecting {
                            error: error.clone(),
                        },
                    ),
                    NativeUpdate::Failed { error } => self.planes.set_connection(
                        self.plane,
                        ConnectionView::Failed {
                            error: error.clone(),
                        },
                    ),
                }
                on_update.send(named(update.into(), self.plane)).is_ok()
            })
            .await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum PlaneUpdate {
    Connected {
        connection: ConnectionSnapshot,
    },
    Resumed {
        after: String,
    },
    Event {
        sequence: String,
        recorded_at_unix_ms: String,
        kind: String,
        conversation_id: Option<String>,
        run_id: Option<String>,
        timeline: Vec<TimelineItemView>,
    },
    Reconnecting {
        error: PublicError,
    },
    Failed {
        error: PublicError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineItemView {
    kind: &'static str,
    text: String,
    item_id: Option<String>,
    approval: Option<ApprovalView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalView {
    request_id: String,
    review_id: Option<String>,
    run_id: Option<String>,
    tool: String,
    action: String,
    target: String,
    scope: &'static str,
    consequence: String,
    rationale: Option<String>,
    state: &'static str,
    can_authorize_retry: bool,
}

impl From<ApprovalProjection> for ApprovalView {
    fn from(value: ApprovalProjection) -> Self {
        Self {
            request_id: value.request_id,
            review_id: value.review_id,
            run_id: value.run_id,
            tool: value.tool,
            action: value.action,
            target: value.target,
            scope: value.scope,
            consequence: value.consequence,
            rationale: value.rationale,
            state: value.state,
            can_authorize_retry: value.can_authorize_retry,
        }
    }
}

impl From<NativeUpdate> for PlaneUpdate {
    fn from(update: NativeUpdate) -> Self {
        match update {
            NativeUpdate::Resumed { after } => Self::Resumed {
                after: after.to_string(),
            },
            NativeUpdate::Event(EventSummary {
                notification: _,
                sequence,
                recorded_at_unix_ms,
                kind,
                conversation_id,
                run_id,
                timeline,
            }) => Self::Event {
                sequence: sequence.to_string(),
                recorded_at_unix_ms: recorded_at_unix_ms.to_string(),
                kind,
                conversation_id: conversation_id.map(|id| id.to_string()),
                run_id: run_id.map(|id| id.to_string()),
                timeline: timeline
                    .into_iter()
                    .map(|item| TimelineItemView {
                        kind: item.kind,
                        text: item.text,
                        item_id: item.item_id,
                        approval: item.approval.map(Into::into),
                    })
                    .collect(),
            },
            NativeUpdate::Reconnecting { error } => Self::Reconnecting { error },
            NativeUpdate::Failed { error } => Self::Failed { error },
        }
    }
}

/// Errors on a feed name the Plane whose feed failed.
fn named(update: PlaneUpdate, plane: PlaneId) -> PlaneUpdate {
    match update {
        PlaneUpdate::Reconnecting { error } => PlaneUpdate::Reconnecting {
            error: error.with_plane(plane.to_string()),
        },
        PlaneUpdate::Failed { error } => PlaneUpdate::Failed {
            error: error.with_plane(plane.to_string()),
        },
        other => other,
    }
}

/// Opens one Plane's read-only feed: a fenced status snapshot plus ordered,
/// redacted updates that resume after the native cursor. It replaces that
/// Plane's previous feed, if any.
pub(crate) async fn open_plane_feed(
    app: tauri::AppHandle,
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    on_update: Channel<PlaneUpdate>,
    after: Option<String>,
    reset: Option<bool>,
) -> Result<ConnectionSnapshot, PublicError> {
    let requested_cursor = parse_resume_cursor(after)?;
    let (binding, client) = bridge.plane(plane_id.as_deref())?;
    let plane = binding.plane;
    // `reset` is the user's Retry: it clears a remote Plane's sticky or
    // backed-off failure. The local Plane connects per request and has no
    // such state, so every open already starts a fresh attempt there.
    if reset.unwrap_or(false) {
        client.reset().await;
    }
    let context = FeedContext {
        app,
        plane,
        feed: Uuid::new_v4(),
        client,
        planes: Arc::clone(&bridge.planes),
        feeds: Arc::clone(&bridge.feeds),
        notifications: Arc::clone(&bridge.notifications),
    };
    context
        .planes
        .set_connection(plane, ConnectionView::Connecting);
    match context.client.status().await {
        Ok(status) => {
            let cursor = requested_cursor.unwrap_or_else(|| status.cursor.unwrap_or_default());
            context.connected(&status);
            let snapshot = ConnectionSnapshot::from_status(context.feed, plane, status);
            let registration = context.registration();
            spawn_feed(registration, async move {
                context.stream(cursor, &on_update).await;
                context.finish();
            });
            Ok(snapshot)
        }
        Err(error) => {
            let public = bridge.settle(&binding, PublicError::from_client(&error));
            if !public.retryable {
                bridge.planes.set_connection(
                    plane,
                    ConnectionView::Failed {
                        error: public.clone(),
                    },
                );
                return Err(public);
            }
            bridge.planes.set_connection(
                plane,
                ConnectionView::Reconnecting {
                    error: public.clone(),
                },
            );
            let snapshot = ConnectionSnapshot::reconnecting(context.feed, plane, &bridge.planes);
            let registration = context.registration();
            spawn_feed(registration, async move {
                reconnect(&context, requested_cursor, public, &on_update).await;
                context.finish();
            });
            Ok(snapshot)
        }
    }
}

/// Keeps reading status until the Plane answers, then streams from it.
async fn reconnect(
    context: &FeedContext,
    requested_cursor: Option<u64>,
    first_error: PublicError,
    on_update: &Channel<PlaneUpdate>,
) {
    if on_update
        .send(PlaneUpdate::Reconnecting { error: first_error })
        .is_err()
    {
        return;
    }
    loop {
        match context.client.status().await {
            Ok(status) => {
                let cursor = requested_cursor.unwrap_or_else(|| status.cursor.unwrap_or_default());
                context.connected(&status);
                let connection =
                    ConnectionSnapshot::from_status(context.feed, context.plane, status);
                if on_update
                    .send(PlaneUpdate::Connected { connection })
                    .is_err()
                {
                    return;
                }
                context.stream(cursor, on_update).await;
                return;
            }
            Err(error) => {
                let error = context
                    .planes
                    .settle(context.plane, PublicError::from_client(&error));
                let terminal = !error.retryable;
                let update = if terminal {
                    context.planes.set_connection(
                        context.plane,
                        ConnectionView::Failed {
                            error: error.clone(),
                        },
                    );
                    PlaneUpdate::Failed { error }
                } else {
                    context.planes.set_connection(
                        context.plane,
                        ConnectionView::Reconnecting {
                            error: error.clone(),
                        },
                    );
                    PlaneUpdate::Reconnecting { error }
                };
                if on_update.send(update).is_err() || terminal {
                    return;
                }
                // A remote Plane waits out its connector's backoff here.
                context.client.pace().await;
            }
        }
    }
}

/// Spawns a feed task and registers it as its Plane's only feed.
fn spawn_feed(
    (feeds, plane, feed): (Arc<FeedRegistry>, PlaneId, Uuid),
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let task = tokio::spawn(task);
    feeds.replace(plane, feed, task.abort_handle());
}

pub(crate) fn close_plane_feed(bridge: &JetBridge, feed_id: String) -> Result<(), PublicError> {
    let feed = Uuid::parse_str(&feed_id).map_err(|_| {
        PublicError::invalid_input("event.feed_invalid", "That activity feed is not valid.")
    })?;
    bridge.feeds.close(feed)
}

fn parse_resume_cursor(after: Option<String>) -> Result<Option<u64>, PublicError> {
    after
        .map(|value| {
            if value.is_empty()
                || value.len() > 20
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(PublicError::invalid_input(
                    "event.cursor_invalid",
                    "Jet could not resume from that activity cursor.",
                ));
            }
            value.parse::<u64>().map_err(|_| {
                PublicError::invalid_input(
                    "event.cursor_invalid",
                    "Jet could not resume from that activity cursor.",
                )
            })
        })
        .transpose()
}

fn bounded_text(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if !value.is_empty()
        && value.len() <= maximum_bytes
        && value
            .chars()
            .all(|character| !character.is_control() && character != '<' && character != '>')
    {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::{bounded_text, named, parse_resume_cursor, FeedRegistry, PlaneUpdate};
    use crate::jet::{errors::PublicError, planes::PlaneId};

    fn pending_task() -> tokio::task::JoinHandle<()> {
        tokio::spawn(async { tokio::time::sleep(Duration::from_secs(3600)).await })
    }

    async fn settled(task: &tokio::task::JoinHandle<()>) -> bool {
        for _ in 0..100 {
            if task.is_finished() {
                return true;
            }
            tokio::task::yield_now().await;
        }
        task.is_finished()
    }

    #[tokio::test]
    async fn opening_a_feed_aborts_only_the_same_planes_previous_feed() {
        let feeds = FeedRegistry::default();
        let remote = PlaneId::Remote(Uuid::from_u128(2));
        let (first, second, other) = (pending_task(), pending_task(), pending_task());
        let (first_id, second_id, other_id) = (
            Uuid::from_u128(10),
            Uuid::from_u128(11),
            Uuid::from_u128(12),
        );
        feeds.replace(PlaneId::Local, first_id, first.abort_handle());
        feeds.replace(remote, other_id, other.abort_handle());
        feeds.replace(PlaneId::Local, second_id, second.abort_handle());
        assert!(settled(&first).await, "the replaced feed must stop polling");
        assert!(!second.is_finished());
        assert!(!other.is_finished());

        // A stale close cannot stop the newer feed of the same Plane.
        feeds.close(first_id).unwrap();
        assert!(!settled(&second).await);
        feeds.close(second_id).unwrap();
        assert!(settled(&second).await);
        assert!(!other.is_finished());

        // A task that ended by itself never removes a newer registration.
        let third = pending_task();
        feeds.replace(remote, Uuid::from_u128(13), third.abort_handle());
        assert!(settled(&other).await);
        feeds.finished(remote, other_id);
        assert_eq!(feeds.by_plane.lock().unwrap().len(), 1);
        feeds.close(Uuid::from_u128(13)).unwrap();
        assert!(settled(&third).await);
        assert!(feeds.by_plane.lock().unwrap().is_empty());
    }

    #[test]
    fn feed_failures_name_their_plane() {
        let update = named(
            PlaneUpdate::Failed {
                error: PublicError::internal(),
            },
            PlaneId::Local,
        );
        let PlaneUpdate::Failed { error } = update else {
            panic!("expected a failure");
        };
        assert_eq!(error.plane_id.as_deref(), Some("local"));
    }

    #[test]
    fn status_text_is_bounded_before_serialization() {
        assert_eq!(bounded_text("0.2.0", 48, "Unknown"), "0.2.0");
        assert_eq!(bounded_text("<script>", 48, "Unknown"), "Unknown");
        assert_eq!(bounded_text(&"x".repeat(49), 48, "Unknown"), "Unknown");
    }

    #[test]
    fn resume_cursor_is_bounded_and_numeric() {
        assert_eq!(parse_resume_cursor(Some("108".into())).unwrap(), Some(108));
        assert!(parse_resume_cursor(Some("1e8".into())).is_err());
        assert!(parse_resume_cursor(Some("9".repeat(21))).is_err());
    }
}
