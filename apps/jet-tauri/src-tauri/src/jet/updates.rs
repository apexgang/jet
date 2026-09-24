//! App updates through the Tauri updater (Wave 4 §B).
//!
//! The plugin is registered only when this build's configuration has
//! `plugins.updater` (the release overlay), and the webview gets no updater
//! permission: the four commands below are the whole surface. Updates are
//! off for a Homebrew-managed Jet (ADR-0026: Homebrew updates it), for a
//! build without updater configuration, and for an install that is not a
//! deb, rpm or AppImage bundle.
//!
//! Every Settings window follows the state through its own watcher
//! (`watch_app_update`), so a window opened during an install, or while the
//! automatic check runs, sees it progress and finish.
//!
//! Privacy: a check fetches `latest.json` from the GitHub release of
//! apexgang/jet, so github.com learns this computer's address and app
//! version. The one automatic check after launch follows the device
//! preference "Check for updates automatically" (on by default,
//! `preferences.rs`); "Check for updates" in Settings always asks first.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{
    ipc::Channel,
    utils::{
        config::{BundleType, PluginConfig},
        platform::bundle_type,
    },
    AppHandle, Manager as _, State,
};
use tokio::{sync::watch, task::AbortHandle};

use super::{
    errors::PublicError,
    local_service::{safe_version, Channel as ServiceChannel, LocalServiceView},
    JetBridge,
};

/// How long after launch the automatic check waits, so it never competes
/// with connecting to the local Plane.
const AUTOMATIC_DELAY: Duration = Duration::from_secs(10);
/// One request for `latest.json`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole download of one bundle (tens of MiB).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Progress is published at most every this many bytes, or every percent
/// when the size is known.
const PROGRESS_STEP: u64 = 256 * 1024;

pub(crate) type BoxFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
/// Called with each downloaded chunk's size and the total, when known.
pub(crate) type Progress = Box<dyn FnMut(u64, Option<u64>) + Send>;
/// A check's answer: a newer release, nothing newer, or why it failed.
pub(crate) type Checked = Result<Option<Arc<dyn PendingUpdate>>, PublicError>;

/// Whether the configuration carries an updater section the plugin can
/// start with: a public key and at least one endpoint. Without one the
/// plugin would fail setup, so development builds never register it.
pub(crate) fn updater_configured(plugins: &PluginConfig) -> bool {
    let Some(updater) = plugins.0.get("updater").and_then(|value| value.as_object()) else {
        return false;
    };
    let pubkey = updater
        .get("pubkey")
        .and_then(|value| value.as_str())
        .is_some_and(|key| !key.trim().is_empty());
    let endpoints = updater
        .get("endpoints")
        .and_then(|value| value.as_array())
        .is_some_and(|endpoints| !endpoints.is_empty());
    pubkey && endpoints
}

/// Registers the updater plugin when the configuration asks for it.
/// Returns whether updates can run in this build.
pub(crate) fn register(app: &AppHandle) -> bool {
    updater_configured(&app.config().plugins)
        && app
            .plugin(tauri_plugin_updater::Builder::new().build())
            .is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DisabledReason {
    /// Homebrew installed and updates this Jet.
    Homebrew,
    /// No updater configuration: a development or local build.
    DevelopmentBuild,
    /// Not running from a deb, rpm or AppImage bundle.
    UnsupportedInstall,
}

/// Why updates are off, if they are.
pub(crate) fn disabled_reason(
    configured: bool,
    installable: bool,
    channel: Option<ServiceChannel>,
) -> Option<DisabledReason> {
    if !configured {
        Some(DisabledReason::DevelopmentBuild)
    } else if !installable {
        Some(DisabledReason::UnsupportedInstall)
    } else if channel == Some(ServiceChannel::Homebrew) {
        Some(DisabledReason::Homebrew)
    } else {
        None
    }
}

/// Bundles the plugin can replace in place.
pub(crate) fn installable(bundle: Option<BundleType>) -> bool {
    matches!(
        bundle,
        Some(BundleType::Deb | BundleType::Rpm | BundleType::AppImage)
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum UpdatePhase {
    Disabled {
        reason: DisabledReason,
    },
    /// `up_to_date`: a check this session found nothing newer.
    Idle {
        up_to_date: bool,
    },
    Checking,
    Available {
        version: String,
        date_unix_ms: Option<String>,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    /// Installed; takes effect when Jet restarts.
    Ready {
        version: String,
    },
    Failed {
        error: PublicError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppUpdateView {
    current_version: &'static str,
    state: UpdatePhase,
}

/// Where updates come from; the plugin in production, scripted in tests.
pub(crate) trait UpdateSource: Send + Sync {
    fn check(&self) -> BoxFuture<'_, Checked>;
}

/// One newer release the source announced.
pub(crate) trait PendingUpdate: Send + Sync {
    /// As announced; checked before it is shown.
    fn version(&self) -> String;
    fn date_unix_ms(&self) -> Option<i64>;
    /// Downloads, verifies the signature, and installs.
    fn install(&self, progress: Progress) -> BoxFuture<'_, Result<(), PublicError>>;
}

#[derive(Clone)]
pub(crate) struct AppUpdateState {
    inner: Arc<Updates>,
}

struct Updates {
    source: Option<Arc<dyn UpdateSource>>,
    installable: bool,
    service: watch::Receiver<LocalServiceView>,
    phase: watch::Sender<UpdatePhase>,
    /// One view watcher per window label (`watch_app_update`).
    watchers: Mutex<HashMap<String, AbortHandle>>,
    pending: Mutex<Option<Arc<dyn PendingUpdate>>>,
    /// One check or install at a time.
    busy: tokio::sync::Mutex<()>,
    restart: Box<dyn Fn() + Send + Sync>,
}

impl AppUpdateState {
    pub(crate) fn new(
        source: Option<Arc<dyn UpdateSource>>,
        installable: bool,
        service: watch::Receiver<LocalServiceView>,
        restart: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(Updates {
                source,
                installable,
                service,
                phase: watch::channel(UpdatePhase::Idle { up_to_date: false }).0,
                watchers: Mutex::new(HashMap::new()),
                pending: Mutex::new(None),
                busy: tokio::sync::Mutex::new(()),
                restart,
            }),
        }
    }

    /// Production: the plugin when `configured`, this binary's bundle type,
    /// and a restart through the event loop (window geometry is saved on
    /// the way out).
    pub(crate) fn for_app(
        app: &AppHandle,
        configured: bool,
        service: watch::Receiver<LocalServiceView>,
    ) -> Self {
        let source = configured
            .then(|| Arc::new(PluginSource { app: app.clone() }) as Arc<dyn UpdateSource>);
        let restart = app.clone();
        Self::new(
            source,
            installable(bundle_type()),
            service,
            Box::new(move || restart.request_restart()),
        )
    }

    fn disabled(&self) -> Option<DisabledReason> {
        let channel = self.inner.service.borrow().channel;
        disabled_reason(self.inner.source.is_some(), self.inner.installable, channel)
    }

    fn phase(&self) -> UpdatePhase {
        self.inner.phase.borrow().clone()
    }

    fn set(&self, phase: UpdatePhase) {
        self.inner.phase.send_if_modified(|current| {
            let changed = *current != phase;
            *current = phase;
            changed
        });
    }

    pub(crate) fn view(&self) -> AppUpdateView {
        let phase = self.phase();
        // An update already on its way or installed still waits for its
        // restart, whatever the service channel says now.
        let underway = matches!(
            phase,
            UpdatePhase::Ready { .. } | UpdatePhase::Downloading { .. }
        );
        let state = match self.disabled() {
            Some(reason) if !underway => UpdatePhase::Disabled { reason },
            _ => phase,
        };
        AppUpdateView {
            current_version: env!("CARGO_PKG_VERSION"),
            state,
        }
    }

    /// Streams view changes to `send` until it returns `false` or the
    /// window's next watch replaces this one; returns the view now. The
    /// view follows this state and the local service's channel (Homebrew
    /// turns updates off).
    pub(crate) fn watch(
        &self,
        window: &str,
        mut send: impl FnMut(AppUpdateView) -> bool + Send + 'static,
    ) -> AppUpdateView {
        let mut phases = self.inner.phase.subscribe();
        let mut service = self.inner.service.clone();
        phases.borrow_and_update();
        service.borrow_and_update();
        let current = self.view();
        let mut last = current.clone();
        let state = self.clone();
        let task = tokio::spawn(async move {
            loop {
                let changed = tokio::select! {
                    changed = phases.changed() => changed,
                    changed = service.changed() => changed,
                };
                if changed.is_err() {
                    break;
                }
                let view = state.view();
                if view != last {
                    last = view.clone();
                    if !send(view) {
                        break;
                    }
                }
            }
        });
        match self.inner.watchers.lock() {
            Ok(mut watchers) => {
                if let Some(previous) = watchers.insert(window.to_owned(), task.abort_handle()) {
                    previous.abort();
                }
            }
            Err(_) => task.abort(),
        }
        current
    }

    /// The window is gone: its watcher has no receiver.
    pub(crate) fn window_destroyed(&self, window: &str) {
        if let Ok(mut watchers) = self.inner.watchers.lock() {
            if let Some(task) = watchers.remove(window) {
                task.abort();
            }
        }
    }

    /// Asks the release endpoint for a newer version. A check while
    /// another runs, or after an update was installed, reports the state.
    pub(crate) async fn check(&self) -> AppUpdateView {
        let (Some(source), None) = (self.inner.source.clone(), self.disabled()) else {
            return self.view();
        };
        let Ok(_busy) = self.inner.busy.try_lock() else {
            return self.view();
        };
        if matches!(self.phase(), UpdatePhase::Ready { .. }) {
            return self.view();
        }
        self.set(UpdatePhase::Checking);
        let phase = match source.check().await {
            Ok(Some(update)) => match safe_version(&update.version()) {
                Some(version) => {
                    let date_unix_ms = update
                        .date_unix_ms()
                        .map(|milliseconds| milliseconds.to_string());
                    self.hold(Some(update));
                    UpdatePhase::Available {
                        version,
                        date_unix_ms,
                    }
                }
                None => {
                    self.hold(None);
                    UpdatePhase::Failed {
                        error: release_invalid(),
                    }
                }
            },
            Ok(None) => {
                self.hold(None);
                UpdatePhase::Idle { up_to_date: true }
            }
            Err(error) => UpdatePhase::Failed { error },
        };
        self.set(phase);
        self.view()
    }

    fn hold(&self, update: Option<Arc<dyn PendingUpdate>>) {
        if let Ok(mut pending) = self.inner.pending.lock() {
            *pending = update;
        }
    }

    /// Downloads and installs the announced update; watchers follow its
    /// progress. Then Jet waits for `restart`.
    pub(crate) async fn install(&self) -> Result<AppUpdateView, PublicError> {
        if self.disabled().is_some() {
            return Err(disabled());
        }
        let _busy = self.inner.busy.try_lock().map_err(|_| busy())?;
        let pending = self
            .inner
            .pending
            .lock()
            .map_err(|_| PublicError::internal())?
            .clone()
            .ok_or_else(not_available)?;
        let version = safe_version(&pending.version()).ok_or_else(release_invalid)?;
        self.set(UpdatePhase::Downloading {
            version: version.clone(),
            downloaded: 0,
            total: None,
        });
        let progress = self.progress(version.clone());
        let phase = match pending.install(progress).await {
            Ok(()) => {
                self.hold(None);
                UpdatePhase::Ready { version }
            }
            // The announced update stays held until the next check, so an
            // install can be tried again without contacting the endpoint.
            Err(error) => UpdatePhase::Failed { error },
        };
        self.set(phase);
        Ok(self.view())
    }

    /// Publishes download progress at most once per step.
    fn progress(&self, version: String) -> Progress {
        let state = self.clone();
        let mut downloaded = 0_u64;
        let mut reported = 0_u64;
        Box::new(move |chunk, total| {
            downloaded = downloaded.saturating_add(chunk);
            let step = total.map_or(PROGRESS_STEP, |total| (total / 100).max(1));
            let finished = total.is_some_and(|total| downloaded >= total);
            if downloaded.saturating_sub(reported) >= step || finished {
                reported = downloaded;
                state.set(UpdatePhase::Downloading {
                    version: version.clone(),
                    downloaded,
                    total,
                });
            }
        })
    }

    /// Restarts Jet into the installed update.
    pub(crate) fn restart(&self) -> Result<(), PublicError> {
        if !matches!(self.phase(), UpdatePhase::Ready { .. }) {
            return Err(not_ready());
        }
        (self.inner.restart)();
        Ok(())
    }
}

/// The automatic check after launch, when the preference allows it.
pub(crate) async fn after_launch(app: &AppHandle) {
    let (Some(updates), Some(bridge)) = (
        app.try_state::<AppUpdateState>(),
        app.try_state::<JetBridge>(),
    ) else {
        return;
    };
    if !bridge.preferences.check_for_updates().unwrap_or(false) {
        return;
    }
    let updates = updates.inner().clone();
    if updates.disabled().is_some() {
        return;
    }
    tokio::time::sleep(AUTOMATIC_DELAY).await;
    let _ = updates.check().await;
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

struct PluginSource {
    app: AppHandle,
}

impl UpdateSource for PluginSource {
    fn check(&self) -> BoxFuture<'_, Checked> {
        use tauri_plugin_updater::UpdaterExt;
        Box::pin(async move {
            let updater = self
                .app
                .updater_builder()
                .timeout(CHECK_TIMEOUT)
                .build()
                .map_err(check_error)?;
            let update = updater.check().await.map_err(check_error)?;
            Ok(update.map(|update| Arc::new(PluginUpdate(update)) as Arc<dyn PendingUpdate>))
        })
    }
}

struct PluginUpdate(tauri_plugin_updater::Update);

impl PendingUpdate for PluginUpdate {
    fn version(&self) -> String {
        self.0.version.clone()
    }

    fn date_unix_ms(&self) -> Option<i64> {
        self.0
            .date
            .map(|date| date.unix_timestamp().saturating_mul(1000))
    }

    fn install(&self, mut progress: Progress) -> BoxFuture<'_, Result<(), PublicError>> {
        Box::pin(async move {
            let mut update = self.0.clone();
            update.timeout = Some(DOWNLOAD_TIMEOUT);
            // The plugin verifies the minisign signature against the
            // configured public key before returning the bytes.
            let bytes = update
                .download(|chunk, total| progress(chunk as u64, total), || {})
                .await
                .map_err(install_error)?;
            // An AppImage is replaced in place. deb and rpm go through the
            // plugin's privilege chain: pkexec, then a zenity or kdialog
            // password with `sudo -S`, then plain `sudo`. A canceled prompt
            // only moves to the next step, so the chain ends in
            // `PackageInstallFailed`, never `AuthenticationFailed`. When Jet
            // was started from a terminal, that last `sudo` asks for the
            // password on the terminal and this install waits for it; Jet
            // cannot interrupt it. Both block.
            tokio::task::spawn_blocking(move || update.install(bytes))
                .await
                .map_err(|_| install_failed())?
                .map_err(install_error)
        })
    }
}

fn check_error(error: tauri_plugin_updater::Error) -> PublicError {
    use tauri_plugin_updater::Error;
    match error {
        Error::Reqwest(_) | Error::Network(_) | Error::Io(_) => offline(),
        // The plugin's answer to any non-2xx reply (a GitHub outage, rate
        // limiting, a release without `latest.json` yet).
        Error::ReleaseNotFound => release_unavailable(),
        Error::Serialization(_)
        | Error::Semver(_)
        | Error::UrlParse(_)
        | Error::TargetNotFound(_)
        | Error::TargetsNotFound(_) => release_invalid(),
        _ => check_failed(),
    }
}

fn install_error(error: tauri_plugin_updater::Error) -> PublicError {
    use tauri_plugin_updater::Error;
    match error {
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion => signature_invalid(),
        // Every privilege step for the deb or rpm failed or was canceled.
        Error::PackageInstallFailed => package_install_failed(),
        Error::Reqwest(_) | Error::Network(_) => download_failed(),
        _ => install_failed(),
    }
}

// ---------------------------------------------------------------------------
// Stable codes
// ---------------------------------------------------------------------------

fn offline() -> PublicError {
    PublicError::unavailable(
        "update.offline",
        "Jet couldn't reach github.com to check for updates.",
        true,
    )
}

fn release_invalid() -> PublicError {
    PublicError::invalid_response_code(
        "update.release_invalid",
        "The update information for this computer couldn't be read.",
    )
}

/// The endpoint answered, but not with the release information.
fn release_unavailable() -> PublicError {
    PublicError::unavailable(
        "update.release_unavailable",
        "The update information isn't available right now. Try again later.",
        true,
    )
}

fn check_failed() -> PublicError {
    PublicError::local_unavailable(
        "update.check_failed",
        "Jet couldn't check for updates.",
        true,
    )
}

fn download_failed() -> PublicError {
    PublicError::unavailable(
        "update.download_failed",
        "The update couldn't be downloaded. Nothing was installed.",
        true,
    )
}

fn signature_invalid() -> PublicError {
    PublicError::invalid_response_code(
        "update.signature_invalid",
        "The downloaded update isn't signed by Jet, so it wasn't installed.",
    )
}

/// The plugin cannot tell a canceled password prompt from a package the
/// system refused.
fn package_install_failed() -> PublicError {
    PublicError::local_unavailable(
        "update.package_install_failed",
        "The update wasn't installed. The password prompt was canceled, or the system refused the package.",
        true,
    )
}

fn install_failed() -> PublicError {
    PublicError::local_unavailable(
        "update.install_failed",
        "The update couldn't be installed.",
        true,
    )
}

fn disabled() -> PublicError {
    PublicError::conflict(
        "update.disabled",
        "Updates for this Jet come from elsewhere.",
    )
}

fn busy() -> PublicError {
    PublicError::unavailable(
        "update.busy",
        "Jet is already checking for or installing an update.",
        true,
    )
}

fn not_available() -> PublicError {
    PublicError::conflict(
        "update.not_available",
        "There is no update to install. Check for updates first.",
    )
}

fn not_ready() -> PublicError {
    PublicError::conflict("update.not_ready", "No update is waiting for a restart.")
}

// ---------------------------------------------------------------------------
// Commands (Settings window)
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn load_app_update(
    state: State<'_, AppUpdateState>,
) -> Result<AppUpdateView, PublicError> {
    Ok(state.view())
}

#[tauri::command]
pub(crate) async fn check_app_update(
    state: State<'_, AppUpdateState>,
) -> Result<AppUpdateView, PublicError> {
    Ok(state.check().await)
}

/// One watcher per window; a second call from the same window replaces it.
#[tauri::command]
pub(crate) async fn watch_app_update(
    window: tauri::Window,
    state: State<'_, AppUpdateState>,
    on_change: Channel<AppUpdateView>,
) -> Result<AppUpdateView, PublicError> {
    Ok(state.watch(window.label(), move |view| on_change.send(view).is_ok()))
}

/// Progress reaches every window through its watcher.
#[tauri::command]
pub(crate) async fn install_app_update(
    state: State<'_, AppUpdateState>,
) -> Result<AppUpdateView, PublicError> {
    state.install().await
}

/// The webview confirms first; the restart closes every window.
#[tauri::command]
pub(crate) async fn restart_after_update(
    state: State<'_, AppUpdateState>,
) -> Result<(), PublicError> {
    state.restart()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::jet::errors::safe_code;

    struct FakeSource {
        answers: Mutex<Vec<Checked>>,
        checks: AtomicUsize,
    }

    impl FakeSource {
        fn new(answers: Vec<Checked>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers),
                checks: AtomicUsize::new(0),
            })
        }
    }

    impl UpdateSource for FakeSource {
        fn check(&self) -> BoxFuture<'_, Checked> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            let answer = self.answers.lock().unwrap().remove(0);
            Box::pin(async move { answer })
        }
    }

    struct FakeUpdate {
        version: &'static str,
        chunks: Vec<u64>,
        total: Option<u64>,
        result: Mutex<Vec<Result<(), PublicError>>>,
        installs: AtomicUsize,
    }

    impl FakeUpdate {
        fn new(version: &'static str, results: Vec<Result<(), PublicError>>) -> Arc<Self> {
            Arc::new(Self {
                version,
                chunks: vec![40, 40, 20],
                total: Some(100),
                result: Mutex::new(results),
                installs: AtomicUsize::new(0),
            })
        }
    }

    impl PendingUpdate for FakeUpdate {
        fn version(&self) -> String {
            self.version.into()
        }

        fn date_unix_ms(&self) -> Option<i64> {
            Some(1_790_000_000_000)
        }

        /// Yields after each chunk, as a download does, so watchers see
        /// each step.
        fn install(&self, mut progress: Progress) -> BoxFuture<'_, Result<(), PublicError>> {
            self.installs.fetch_add(1, Ordering::SeqCst);
            let result = self.result.lock().unwrap().remove(0);
            Box::pin(async move {
                for chunk in &self.chunks {
                    progress(*chunk, self.total);
                    tokio::task::yield_now().await;
                }
                result
            })
        }
    }

    /// Lets spawned watchers run.
    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    type Seen = Arc<Mutex<Vec<UpdatePhase>>>;

    /// A window's watcher that records every state it is sent.
    fn watcher(updates: &AppUpdateState, window: &str) -> (UpdatePhase, Seen) {
        let seen = Seen::default();
        let sink = seen.clone();
        let initial = updates.watch(window, move |view| {
            sink.lock().unwrap().push(view.state);
            true
        });
        (initial.state, seen)
    }

    fn service(channel: Option<ServiceChannel>) -> watch::Sender<LocalServiceView> {
        let mut view = LocalServiceView::checking(None);
        view.channel = channel;
        watch::channel(view).0
    }

    fn state(
        source: Option<Arc<dyn UpdateSource>>,
        service: &watch::Sender<LocalServiceView>,
        restarts: Arc<AtomicUsize>,
    ) -> AppUpdateState {
        AppUpdateState::new(
            source,
            true,
            service.subscribe(),
            Box::new(move || {
                restarts.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    #[test]
    fn updates_are_disabled_for_the_right_reason() {
        let homebrew = Some(ServiceChannel::Homebrew);
        let gui = Some(ServiceChannel::Gui);
        let cases = [
            (false, true, gui, Some(DisabledReason::DevelopmentBuild)),
            (
                false,
                false,
                homebrew,
                Some(DisabledReason::DevelopmentBuild),
            ),
            (true, false, gui, Some(DisabledReason::UnsupportedInstall)),
            (true, true, homebrew, Some(DisabledReason::Homebrew)),
            (true, true, gui, None),
            (true, true, Some(ServiceChannel::Development), None),
            (true, true, None, None),
        ];
        for (configured, installable, channel, expected) in cases {
            assert_eq!(
                disabled_reason(configured, installable, channel),
                expected,
                "{configured} {installable} {channel:?}"
            );
        }
        assert!(installable(Some(BundleType::Deb)));
        assert!(installable(Some(BundleType::Rpm)));
        assert!(installable(Some(BundleType::AppImage)));
        assert!(!installable(None));
        assert!(!installable(Some(BundleType::Dmg)));
    }

    /// The seam `lib.rs` uses: only a usable `plugins.updater` registers the
    /// plugin, so development builds without the release overlay start.
    #[test]
    fn only_a_complete_updater_section_registers_the_plugin() {
        let config = |plugins: serde_json::Value| -> PluginConfig {
            serde_json::from_value(plugins).unwrap()
        };
        let release = json!({"updater": {
            "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6",
            "endpoints": ["https://github.com/apexgang/jet/releases/latest/download/latest.json"]
        }});
        assert!(updater_configured(&config(release)));
        for missing in [
            json!({}),
            json!({"updater": null}),
            json!({"updater": {"endpoints": ["https://example.invalid/latest.json"]}}),
            json!({"updater": {"pubkey": " ", "endpoints": ["https://example.invalid"]}}),
            json!({"updater": {"pubkey": "k", "endpoints": []}}),
            json!({"notification": {}}),
        ] {
            assert!(!updater_configured(&config(missing.clone())), "{missing}");
        }
        // This repository's own configuration is a development build.
        let own: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let plugins = own.get("plugins").cloned().unwrap_or_else(|| json!({}));
        assert!(!updater_configured(&config(plugins)));
    }

    #[tokio::test]
    async fn check_install_and_restart() {
        let service = service(Some(ServiceChannel::Gui));
        let update = FakeUpdate::new("0.3.0", vec![Ok(())]);
        let source = FakeSource::new(vec![Ok(Some(update.clone()))]);
        let restarts = Arc::new(AtomicUsize::new(0));
        let updates = state(Some(source.clone()), &service, restarts.clone());

        assert_eq!(
            updates.view().state,
            UpdatePhase::Idle { up_to_date: false }
        );
        assert_eq!(updates.restart().unwrap_err().code, "update.not_ready");
        let available = updates.check().await;
        assert_eq!(
            available.state,
            UpdatePhase::Available {
                version: "0.3.0".into(),
                date_unix_ms: Some("1790000000000".into()),
            }
        );
        let json = serde_json::to_value(&available).unwrap();
        assert_eq!(json["currentVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["state"]["kind"], "available");
        assert_eq!(json["state"]["dateUnixMs"], "1790000000000");

        let (_, sent) = watcher(&updates, "settings");
        let done = updates.install().await.unwrap();
        assert_eq!(
            done.state,
            UpdatePhase::Ready {
                version: "0.3.0".into()
            }
        );
        settle().await;
        let sent = sent.lock().unwrap().clone();
        let downloading = |downloaded| UpdatePhase::Downloading {
            version: "0.3.0".into(),
            downloaded,
            total: Some(100),
        };
        assert_eq!(
            sent,
            [
                downloading(40),
                downloading(80),
                downloading(100),
                UpdatePhase::Ready {
                    version: "0.3.0".into()
                }
            ]
        );

        // Installed: a later check does not contact the endpoint again, and
        // there is nothing left to install.
        assert_eq!(
            updates.check().await.state,
            UpdatePhase::Ready {
                version: "0.3.0".into()
            }
        );
        assert_eq!(source.checks.load(Ordering::SeqCst), 1);
        assert_eq!(
            updates.install().await.unwrap_err().code,
            "update.not_available"
        );

        updates.restart().unwrap();
        assert_eq!(restarts.load(Ordering::SeqCst), 1);
    }

    /// A window follows checks and installs it did not start (the
    /// automatic check, an install from a window since closed) and the
    /// Homebrew channel, until it closes or watches again.
    #[tokio::test]
    async fn every_window_follows_the_update_state() {
        let service = service(Some(ServiceChannel::Gui));
        let update = FakeUpdate::new("0.3.0", vec![Ok(())]);
        let source = FakeSource::new(vec![Ok(Some(update))]);
        let updates = state(Some(source), &service, Arc::new(AtomicUsize::new(0)));
        let (initial, seen) = watcher(&updates, "settings");
        assert_eq!(initial, UpdatePhase::Idle { up_to_date: false });
        let last = |seen: &Seen| seen.lock().unwrap().last().cloned();
        let available = UpdatePhase::Available {
            version: "0.3.0".into(),
            date_unix_ms: Some("1790000000000".into()),
        };

        // The automatic check after launch.
        updates.check().await;
        settle().await;
        assert_eq!(last(&seen), Some(available.clone()));

        // Homebrew took over the service, then let it go.
        service.send_modify(|view| view.channel = Some(ServiceChannel::Homebrew));
        settle().await;
        assert_eq!(
            last(&seen),
            Some(UpdatePhase::Disabled {
                reason: DisabledReason::Homebrew
            })
        );
        service.send_modify(|view| view.channel = Some(ServiceChannel::Gui));
        settle().await;
        assert_eq!(last(&seen), Some(available));

        // Watching again from the same window replaces the first watcher,
        // which then sees nothing more.
        let (again, current) = watcher(&updates, "settings");
        assert!(matches!(again, UpdatePhase::Available { .. }));
        let before = seen.lock().unwrap().len();
        updates.install().await.unwrap();
        settle().await;
        assert_eq!(seen.lock().unwrap().len(), before);
        assert_eq!(
            last(&current),
            Some(UpdatePhase::Ready {
                version: "0.3.0".into()
            })
        );

        updates.window_destroyed("settings");
        let sent = current.lock().unwrap().len();
        service.send_modify(|view| view.previous_version = Some("0.1.0".into()));
        updates.set(UpdatePhase::Idle { up_to_date: true });
        settle().await;
        assert_eq!(current.lock().unwrap().len(), sent);
    }

    #[tokio::test]
    async fn failures_are_stable_codes_and_retryable() {
        let service = service(None);
        let update = FakeUpdate::new("0.3.0", vec![Err(signature_invalid()), Ok(())]);
        let source = FakeSource::new(vec![Err(offline()), Ok(None), Ok(Some(update.clone()))]);
        let updates = state(Some(source), &service, Arc::new(AtomicUsize::new(0)));

        match updates.check().await.state {
            UpdatePhase::Failed { error } => assert_eq!(error.code, "update.offline"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            updates.check().await.state,
            UpdatePhase::Idle { up_to_date: true }
        );
        assert!(matches!(
            updates.check().await.state,
            UpdatePhase::Available { .. }
        ));

        match updates.install().await.unwrap().state {
            UpdatePhase::Failed { error } => assert_eq!(error.code, "update.signature_invalid"),
            other => panic!("{other:?}"),
        }
        // The announced update is kept for "Try again".
        assert_eq!(
            updates.install().await.unwrap().state,
            UpdatePhase::Ready {
                version: "0.3.0".into()
            }
        );
        assert_eq!(update.installs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_unsafe_announced_version_is_never_shown() {
        let service = service(None);
        let update = FakeUpdate::new("1.0.0 <b>", vec![]);
        let source = FakeSource::new(vec![Ok(Some(update))]);
        let updates = state(Some(source), &service, Arc::new(AtomicUsize::new(0)));
        match updates.check().await.state {
            UpdatePhase::Failed { error } => assert_eq!(error.code, "update.release_invalid"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            updates.install().await.unwrap_err().code,
            "update.not_available"
        );
    }

    #[tokio::test]
    async fn homebrew_and_development_builds_never_check() {
        let service = service(Some(ServiceChannel::Homebrew));
        let source = FakeSource::new(vec![]);
        let updates = state(
            Some(source.clone()),
            &service,
            Arc::new(AtomicUsize::new(0)),
        );
        assert_eq!(
            updates.check().await.state,
            UpdatePhase::Disabled {
                reason: DisabledReason::Homebrew
            }
        );
        assert_eq!(updates.install().await.unwrap_err().code, "update.disabled");
        assert_eq!(source.checks.load(Ordering::SeqCst), 0);

        // The channel is followed live: Homebrew stopped managing it.
        service.send_modify(|view| view.channel = Some(ServiceChannel::Gui));
        assert_eq!(
            updates.view().state,
            UpdatePhase::Idle { up_to_date: false }
        );

        let development = state(None, &service, Arc::new(AtomicUsize::new(0)));
        assert_eq!(
            development.check().await.state,
            UpdatePhase::Disabled {
                reason: DisabledReason::DevelopmentBuild
            }
        );
        let json = serde_json::to_value(development.view()).unwrap();
        assert_eq!(
            json["state"],
            json!({"kind": "disabled", "reason": "development_build"})
        );
    }

    #[test]
    fn update_codes_are_safe() {
        for error in [
            offline(),
            release_invalid(),
            check_failed(),
            download_failed(),
            signature_invalid(),
            release_unavailable(),
            package_install_failed(),
            install_failed(),
            disabled(),
            busy(),
            not_available(),
            not_ready(),
        ] {
            assert!(error.code.starts_with("update."));
            assert_eq!(safe_code(&error.code).as_deref(), Some(error.code.as_str()));
        }
        // Any non-2xx reply from the endpoint: worth trying again.
        let not_found = check_error(tauri_plugin_updater::Error::ReleaseNotFound);
        assert_eq!(
            (not_found.code.as_str(), not_found.retryable),
            ("update.release_unavailable", true)
        );
        let unreadable = check_error(tauri_plugin_updater::Error::Semver(
            semver::Version::parse("x").unwrap_err(),
        ));
        assert_eq!(
            (unreadable.code.as_str(), unreadable.retryable),
            ("update.release_invalid", false)
        );
        assert_eq!(
            check_error(tauri_plugin_updater::Error::Network("x".into())).code,
            "update.offline"
        );
        assert_eq!(
            install_error(tauri_plugin_updater::Error::MissingSignedVersion).code,
            "update.signature_invalid"
        );
        // A canceled or failed password prompt, or a refused package.
        assert_eq!(
            install_error(tauri_plugin_updater::Error::PackageInstallFailed).code,
            "update.package_install_failed"
        );
        assert_eq!(
            install_error(tauri_plugin_updater::Error::DebInstallFailed).code,
            "update.install_failed"
        );
    }
}
