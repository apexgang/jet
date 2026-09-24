//! The local Jet service on this computer (ADR-0026, Wave 4 §A).
//!
//! At launch, and when the user asks for a repair, one provisioning pass
//! observes the local core natively, runs the pure decision table
//! (`decision.rs`) and carries out its plan: nothing for a daemon another
//! channel manages, an update of an older GUI-managed one, a start of a
//! stopped one, or an install from the payload this build carries. After
//! any start it waits a bounded time for the socket and observes again.
//!
//! The webview reads `LocalServiceView` and may ask for a repair or a
//! reviewed rollback; it never names a path, a version or a command. Passes
//! are serialized in this process and, through `local-service.lock` in the
//! app data directory, across app instances: `jetd core` uses fixed
//! temporary names and must never run twice at once.
mod codes;
mod core_cli;
mod decision;
mod homebrew;
pub(crate) mod launcher;
mod manager;
mod payload;
pub(crate) mod process;
#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    fs::{self, OpenOptions, TryLockError},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{ipc::Channel as IpcChannel, path::BaseDirectory, AppHandle, Manager as _, State};
use tokio::{sync::watch, task::AbortHandle};
use uuid::Uuid;

pub(crate) use self::{
    core_cli::{safe_version, Channel},
    decision::LocalServiceView,
};
use self::{
    core_cli::{CoreStatus, Owner},
    decision::{Action, Facts, Outcome, OwnerFact, Phase, Plan, Release, Via},
    homebrew::Keg,
    payload::{BundledPayload, Extracted},
    process::{Clock, Processes, Reachability},
};
use super::{errors::PublicError, JetBridge};

/// How long a started daemon has to answer on its socket.
const START_DEADLINE: Duration = Duration::from_secs(15);
/// One socket probe: connect, local handshake and `status`, including the
/// client's one reconnect after 1 s. A daemon that accepts but never answers
/// (stopped, wedged, migrating before its handshake) counts as unreachable.
const PROBE_LIMIT: Duration = Duration::from_secs(5);
/// Between socket probes and lock attempts.
const POLL: Duration = Duration::from_millis(500);
/// How long a pass waits for another app instance's pass.
const LOCK_DEADLINE: Duration = Duration::from_secs(60);
/// A rollback review is usable once, within this time (as `ledger.rs`).
const REVIEW_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// Cross-process serialization, in the app data directory.
pub(crate) const LOCK_FILE: &str = "local-service.lock";

/// Where the local service lives and which tools manage it. Everything is
/// resolved natively at launch; tests point it at a scratch directory.
#[derive(Debug, Clone)]
pub(crate) struct ServiceSettings {
    /// The Jet home, `~/.jet` (the socket's home, `jet/mod.rs`).
    pub(crate) jet_home: PathBuf,
    /// The user's home, for the service `PATH`.
    pub(crate) user_home: PathBuf,
    /// `$XDG_CONFIG_HOME`: the unit and the autostart file live under it.
    pub(crate) config_home: PathBuf,
    /// The app cache directory; payloads are unpacked under it.
    pub(crate) scratch: PathBuf,
    pub(crate) lock_file: PathBuf,
    pub(crate) bundled: Option<BundledPayload>,
    pub(crate) homebrew_prefixes: Vec<PathBuf>,
    /// Keg executables must be owned by this user or root.
    pub(crate) owner_uid: u32,
    pub(crate) systemctl: Option<PathBuf>,
    pub(crate) tar: Option<PathBuf>,
}

impl ServiceSettings {
    /// Production paths. Nothing here can fail launch: a directory the
    /// platform cannot name falls back to its XDG default.
    pub(crate) fn for_app(app: &AppHandle, user_home: &Path, app_data_directory: &Path) -> Self {
        let resolver = app.path();
        let bundled = payload::build_target().and_then(|target| {
            resolver
                .resolve(payload::RESOURCE, BaseDirectory::Resource)
                .ok()
                .map(|archive| BundledPayload {
                    archive,
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    target: target.to_owned(),
                })
        });
        Self {
            jet_home: user_home.join(".jet"),
            user_home: user_home.to_owned(),
            config_home: resolver
                .config_dir()
                .unwrap_or_else(|_| user_home.join(".config")),
            scratch: resolver
                .app_cache_dir()
                .unwrap_or_else(|_| user_home.join(".cache").join(launcher::APP_ID)),
            lock_file: app_data_directory.join(LOCK_FILE),
            bundled,
            homebrew_prefixes: homebrew::prefixes(std::env::var_os("HOMEBREW_PREFIX"), user_home),
            // The bridge created the app data directory, so it is ours.
            owner_uid: fs::metadata(app_data_directory)
                .map(|metadata| metadata.uid())
                .unwrap_or(u32::MAX),
            systemctl: manager::locate(&["/usr/bin/systemctl", "/bin/systemctl"]),
            tar: manager::locate(&["/usr/bin/tar", "/bin/tar"]),
        }
    }

    fn current_jetd(&self) -> PathBuf {
        self.jet_home.join("core/current/jetd")
    }

    fn unit_path(&self) -> PathBuf {
        manager::unit_path(&self.config_home)
    }

    fn autostart_path(&self) -> PathBuf {
        manager::autostart_path(&self.config_home)
    }

    fn bundled_version(&self) -> Option<String> {
        self.bundled
            .as_ref()
            .filter(|bundled| bundled.present())
            .map(|bundled| bundled.version.clone())
    }
}

/// Managed state: the local service of this computer.
#[derive(Clone)]
pub(crate) struct LocalServiceState {
    inner: Arc<Service>,
}

struct Service {
    settings: ServiceSettings,
    processes: Arc<dyn Processes>,
    reachability: Arc<dyn Reachability>,
    clock: Arc<dyn Clock>,
    view: watch::Sender<LocalServiceView>,
    /// One pass at a time in this process.
    pass: tokio::sync::Mutex<()>,
    /// One view watcher per window label (`watch_local_service`).
    watchers: Mutex<HashMap<String, AbortHandle>>,
    review: Mutex<Option<RollbackReview>>,
}

/// The only rollback this app may send: the pair a fresh status showed.
#[derive(Debug, Clone)]
struct RollbackReview {
    id: Uuid,
    current: String,
    previous: String,
    issued: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RollbackReviewView {
    review_id: String,
    /// The version running now, which the rollback leaves.
    current_version: String,
    /// The version `jetd core rollback` switches back to.
    previous_version: String,
}

impl LocalServiceState {
    pub(crate) fn new(
        settings: ServiceSettings,
        processes: Arc<dyn Processes>,
        reachability: Arc<dyn Reachability>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let (view, _) = watch::channel(LocalServiceView::checking(settings.bundled_version()));
        Self {
            inner: Arc::new(Service {
                settings,
                processes,
                reachability,
                clock,
                view,
                pass: tokio::sync::Mutex::new(()),
                watchers: Mutex::new(HashMap::new()),
                review: Mutex::new(None),
            }),
        }
    }

    /// Production: real processes and clock, and the bridge's own local
    /// Plane client for the socket check (no second transport).
    pub(crate) fn for_app(
        app: &AppHandle,
        bridge: &JetBridge,
        user_home: &Path,
        app_data_directory: &Path,
    ) -> Self {
        Self::new(
            ServiceSettings::for_app(app, user_home, app_data_directory),
            Arc::new(process::SystemProcesses),
            Arc::new(process::LocalPlane(bridge.local().clone())),
            Arc::new(process::SystemClock),
        )
    }

    pub(crate) fn view(&self) -> LocalServiceView {
        self.inner.view.borrow().clone()
    }

    /// Every later view, for the app updater (the Homebrew channel turns it
    /// off).
    pub(crate) fn subscribe(&self) -> watch::Receiver<LocalServiceView> {
        self.inner.view.subscribe()
    }

    /// Streams view changes to `send` until it returns `false` or the
    /// window's next watch replaces this one; returns the view now.
    pub(crate) fn watch(
        &self,
        window: &str,
        mut send: impl FnMut(LocalServiceView) -> bool + Send + 'static,
    ) -> LocalServiceView {
        let mut changes = self.inner.view.subscribe();
        let current = changes.borrow_and_update().clone();
        let task = tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let view = changes.borrow_and_update().clone();
                if !send(view) {
                    break;
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

    /// One provisioning pass; waits for a pass already running.
    pub(crate) async fn provision(&self) -> LocalServiceView {
        let service = &self.inner;
        let _pass = service.pass.lock().await;
        service.publish(service.current().working(Phase::Checking));
        let view = match service.lock_across_instances().await {
            Ok(_lock) => Run::new(service).provision().await,
            Err(error) => LocalServiceView {
                can_repair: true,
                error: Some(error),
                ..service.current().working(Phase::Failed)
            },
        };
        service.publish(view.clone());
        view
    }

    /// Reviews a rollback after a fresh status read: only a GUI-managed
    /// core with a `previous` version.
    pub(crate) async fn prepare_rollback(&self) -> Result<RollbackReviewView, PublicError> {
        let service = &self.inner;
        let _pass = service.pass.try_lock().map_err(|_| codes::busy())?;
        let status = service.rollback_status().await?;
        let (Some(current), Some(previous)) = (status.current, status.previous) else {
            return Err(codes::rollback_unavailable());
        };
        let id = Uuid::new_v4();
        *service.review.lock().map_err(|_| PublicError::internal())? = Some(RollbackReview {
            id,
            current: current.clone(),
            previous: previous.clone(),
            issued: service.clock.now(),
        });
        Ok(RollbackReviewView {
            review_id: id.to_string(),
            current_version: current,
            previous_version: previous,
        })
    }

    /// Sends a reviewed rollback at most once. `Err` means nothing changed;
    /// after the switch the view carries any start failure.
    pub(crate) async fn execute_rollback(
        &self,
        review_id: &str,
    ) -> Result<LocalServiceView, PublicError> {
        let service = &self.inner;
        let id = Uuid::parse_str(review_id).map_err(|_| codes::review_expired())?;
        let review = service.take_review(id)?;
        let _pass = service.pass.lock().await;
        let before = service.current();
        service.publish(before.working(Phase::Updating));
        let _lock = match service.lock_across_instances().await {
            Ok(lock) => lock,
            Err(error) => {
                service.publish(before);
                return Err(error);
            }
        };
        let mut run = Run::new(service);
        let (outcome, returned) = match run.rollback(&review).await {
            Ok(outcome) => (outcome, None),
            Err(unchanged) => (Outcome::default(), Some(unchanged)),
        };
        let after = run.observe().await;
        let view = decision::settle(
            &after.facts,
            &Outcome {
                channel: after.facts.current.as_ref().map(|_| Channel::Gui),
                ..outcome
            },
        );
        service.publish(view.clone());
        match returned {
            Some(error) => Err(error),
            None => Ok(view),
        }
    }
}

impl Service {
    fn current(&self) -> LocalServiceView {
        self.view.borrow().clone()
    }

    fn publish(&self, view: LocalServiceView) {
        self.view.send_if_modified(|current| {
            let changed = *current != view;
            *current = view;
            changed
        });
    }

    /// The cross-process lock, held until the returned file is dropped.
    async fn lock_across_instances(&self) -> Result<fs::File, PublicError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&self.settings.lock_file)
            .map_err(|_| codes::lock_unavailable())?;
        let started = self.clock.now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(_)) => return Err(codes::lock_unavailable()),
            }
            if self.clock.now().saturating_duration_since(started) >= LOCK_DEADLINE {
                return Err(codes::busy());
            }
            self.clock.sleep(POLL).await;
        }
    }

    /// A status from `current/jetd` that allows a rollback: the core is
    /// GUI-managed (a layout exists and no other channel holds the Plane).
    async fn rollback_status(&self) -> Result<CoreStatus, PublicError> {
        let jetd = self.settings.current_jetd();
        if !is_file(&jetd) {
            return Err(codes::rollback_unavailable());
        }
        let status = core_cli::status(&*self.processes, &jetd, &self.settings.jet_home).await?;
        match status.owner {
            Owner::Free
            | Owner::Held {
                channel: Some(Channel::Gui),
                ..
            } => Ok(status),
            Owner::Held { channel: None, .. } => Err(codes::owner_unknown()),
            Owner::Held { .. } => Err(codes::channel_owned()),
        }
    }

    /// Takes the review `id` if it is the one issued and still fresh. A
    /// different ID leaves the issued review in place.
    fn take_review(&self, id: Uuid) -> Result<RollbackReview, PublicError> {
        let mut held = self.review.lock().map_err(|_| PublicError::internal())?;
        match held.take() {
            Some(review) if review.id == id => {
                if self.clock.now().saturating_duration_since(review.issued) > REVIEW_LIFETIME {
                    Err(codes::review_expired())
                } else {
                    Ok(review)
                }
            }
            other => {
                *held = other;
                Err(codes::review_expired())
            }
        }
    }

    /// One bounded socket probe; no pass waits on a silent daemon forever.
    async fn probe(&self) -> bool {
        self.clock
            .within(PROBE_LIMIT, self.reachability.reachable())
            .await
            .unwrap_or(false)
    }

    async fn wait_until_reachable(&self) -> Result<(), PublicError> {
        let started = self.clock.now();
        loop {
            if self.probe().await {
                return Ok(());
            }
            if self.clock.now().saturating_duration_since(started) >= START_DEADLINE {
                return Err(codes::start_timeout());
            }
            self.clock.sleep(POLL).await;
        }
    }
}

fn is_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// `jetd core` found a daemon holding the Plane after this pass saw it free.
fn plane_taken(error: &PublicError) -> bool {
    [
        codes::channel_owned(),
        codes::owner_unknown(),
        codes::drain_timeout(),
    ]
    .iter()
    .any(|taken| taken.code == error.code)
}

/// What one observation found, and a problem that kept `jetd core status`
/// from answering.
struct Observed {
    facts: Facts,
    problem: Option<PublicError>,
}

/// One provisioning pass, or one rollback. Holds the unpacked payload until
/// it ends (dropping it removes the directory) and caches what does not
/// change within a pass.
struct Run<'a> {
    service: &'a Service,
    extracted: Option<Extracted>,
    systemd: Option<bool>,
    keg: Option<Option<Keg>>,
}

impl<'a> Run<'a> {
    fn new(service: &'a Service) -> Self {
        payload::clear_stale(&service.settings.scratch);
        Self {
            service,
            extracted: None,
            systemd: None,
            keg: None,
        }
    }

    fn settings(&self) -> &'a ServiceSettings {
        &self.service.settings
    }

    fn processes(&self) -> &'a dyn Processes {
        &*self.service.processes
    }

    fn keg(&mut self) -> Option<Keg> {
        let settings = self.settings();
        self.keg
            .get_or_insert_with(|| homebrew::find(&settings.homebrew_prefixes, settings.owner_uid))
            .clone()
    }

    async fn systemd(&mut self) -> bool {
        if let Some(available) = self.systemd {
            return available;
        }
        let available =
            manager::systemd_available(self.processes(), self.settings().systemctl.as_deref())
                .await;
        self.systemd = Some(available);
        available
    }

    /// The bundled payload, unpacked once per pass.
    async fn payload(&mut self) -> Result<&Extracted, PublicError> {
        if self.extracted.is_none() {
            let settings = self.settings();
            let bundled = settings
                .bundled
                .as_ref()
                .filter(|bundled| bundled.present())
                .ok_or_else(codes::payload_invalid)?;
            let extracted = payload::extract(
                self.processes(),
                settings.tar.as_deref(),
                bundled,
                &settings.scratch,
            )
            .await?;
            self.extracted = Some(extracted);
        }
        self.extracted.as_ref().ok_or_else(PublicError::internal)
    }

    /// Status from the first `jetd` available: `current`, the Homebrew keg,
    /// then the bundled payload.
    async fn observe(&mut self) -> Observed {
        let settings = self.settings();
        let keg = self.keg();
        let bundled = settings
            .bundled
            .as_ref()
            .filter(|bundled| bundled.present());
        let mut problem = None;
        let current = settings.current_jetd();
        let jetd = if is_file(&current) {
            Some(current)
        } else if let Some(keg) = &keg {
            Some(keg.jetd.clone())
        } else if bundled.is_some() {
            match self.payload().await {
                Ok(extracted) => Some(extracted.jetd.clone()),
                Err(error) => {
                    problem = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let status = match jetd {
            Some(jetd) => {
                match core_cli::status(self.processes(), &jetd, &settings.jet_home).await {
                    Ok(status) => Some(status),
                    Err(error) => {
                        problem.get_or_insert(error);
                        None
                    }
                }
            }
            None => None,
        };
        let reachable = self.service.probe().await;
        let systemd = self.systemd().await;
        let (owner, running) = match status.as_ref().map(|status| &status.owner) {
            None => (OwnerFact::Unknown, None),
            Some(Owner::Free) => (OwnerFact::Free, None),
            Some(Owner::Held { channel, version }) => (OwnerFact::Held(*channel), version.clone()),
        };
        let facts = Facts {
            owner,
            running,
            current: status
                .as_ref()
                .and_then(|status| status.current.clone())
                .map(|version| Release {
                    version,
                    target: payload::current_target(&settings.jet_home),
                }),
            previous: status.and_then(|status| status.previous),
            unit_file: manager::exists(&settings.unit_path()),
            autostart_file: manager::exists(&settings.autostart_path()),
            systemd,
            keg: keg.is_some(),
            bundled: bundled.map(|bundled| Release {
                version: bundled.version.clone(),
                target: Some(bundled.target.clone()),
            }),
            reachable,
        };
        Observed { facts, problem }
    }

    async fn provision(&mut self) -> LocalServiceView {
        let observed = self.observe().await;
        let plan = decision::decide(&observed.facts);
        let mut outcome = Outcome {
            channel: plan.channel(),
            not_installed: plan == Plan::NotInstalled,
            ..Outcome::default()
        };
        match observed.problem {
            // Nothing answers and nothing could tell who owns the Plane:
            // acting now could start a second manager.
            Some(problem) if !observed.facts.reachable => outcome.error = Some(problem),
            _ => match self.execute(&plan, &observed.facts).await {
                Ok(action) => outcome.action = action,
                Err(error) => outcome.error = Some(error),
            },
        }
        let after = self.observe().await;
        decision::settle(&after.facts, &outcome)
    }

    async fn execute(&mut self, plan: &Plan, facts: &Facts) -> Result<Option<Action>, PublicError> {
        let service = self.service;
        match plan {
            Plan::Keep {
                channel,
                ensure_unit,
            } => {
                if *channel == Some(Channel::Gui) && facts.systemd {
                    // Best effort: for later logins only; never restarts.
                    if *ensure_unit || facts.unit_file {
                        let _ = self.register_unit(false).await;
                    }
                }
                if !facts.reachable {
                    service.wait_until_reachable().await?;
                }
                Ok(None)
            }
            Plan::Update { version, via } => {
                service.publish(service.current().working(Phase::Updating));
                if *via == Via::Systemd {
                    let _ = self.refresh_unit().await;
                }
                self.stage_and_activate(version).await?;
                // Activation drained the daemon; systemd restarts it
                // (`Restart=always`), otherwise this app starts it.
                if *via == Via::Spawn {
                    self.spawn()?;
                }
                service.wait_until_reachable().await?;
                Ok(Some(Action::Updated))
            }
            Plan::Start { update, via } => {
                let mut update_failed = None;
                if let Some(version) = update {
                    service.publish(service.current().working(Phase::Updating));
                    if *via == Via::Systemd {
                        let _ = self.refresh_unit().await;
                    }
                    update_failed = self.try_update(version).await?;
                }
                service.publish(service.current().working(Phase::Starting));
                match via {
                    Via::Systemd => self.systemd_start().await?,
                    Via::Spawn => self.spawn()?,
                }
                service.wait_until_reachable().await?;
                match update_failed {
                    // The earlier `current` serves; the pass still says why
                    // it was not updated.
                    Some(error) => Err(error),
                    None if update.is_some() => Ok(Some(Action::Updated)),
                    None => Ok(Some(Action::Started)),
                }
            }
            Plan::BrewStart => {
                service.publish(service.current().working(Phase::Starting));
                let keg = self.keg().ok_or_else(codes::homebrew_start_failed)?;
                let started = self
                    .processes()
                    .run(
                        &manager::brew_services_start(&keg.brew),
                        manager::BREW_LIMIT,
                    )
                    .await
                    .is_ok_and(|finished| finished.succeeded());
                if !started {
                    return Err(codes::homebrew_start_failed());
                }
                service.wait_until_reachable().await?;
                Ok(Some(Action::Started))
            }
            Plan::Install { activate, via } => {
                service.publish(service.current().working(Phase::Installing));
                let mut update_failed = None;
                match activate {
                    // An older `current` without a service file is still
                    // registered and started when the update fails.
                    Some(version) if facts.current.is_some() => {
                        update_failed = self.try_update(version).await?;
                    }
                    Some(version) => self.stage_and_activate(version).await?,
                    None => {}
                }
                match via {
                    Via::Systemd => self.register_unit(true).await?,
                    Via::Spawn => {
                        let path = self.settings().autostart_path();
                        blocking(move || {
                            manager::write_if_changed(
                                &path,
                                manager::AUTOSTART_TEMPLATE.as_bytes(),
                                0o644,
                            )
                        })
                        .await?
                        .map_err(|_| codes::install_failed())?;
                        self.spawn()?;
                    }
                }
                service.wait_until_reachable().await?;
                match update_failed {
                    Some(error) => Err(error),
                    None => Ok(Some(Action::Installed)),
                }
            }
            Plan::NotInstalled => Ok(None),
        }
    }

    /// Updates a stopped installation before it is started. A failed update
    /// leaves `current` startable (`jetd core activate` refuses before it
    /// drains, and switches the links last), so the failure is returned as
    /// `Ok(Some(..))` for the caller to report after the start. It stays an
    /// `Err` when another daemon holds the Plane now: a second start would
    /// only compete for its lock.
    async fn try_update(&mut self, version: &str) -> Result<Option<PublicError>, PublicError> {
        match self.stage_and_activate(version).await {
            Ok(()) => Ok(None),
            Err(error) if plane_taken(&error) => Err(error),
            Err(error) => Ok(Some(error)),
        }
    }

    /// Stages the bundled payload and activates `version` with its `jetd`.
    async fn stage_and_activate(&mut self, version: &str) -> Result<(), PublicError> {
        let home = self.settings().jet_home.clone();
        let (jetd, directory) = {
            let extracted = self.payload().await?;
            (extracted.jetd.clone(), extracted.payload.clone())
        };
        let staged = core_cli::stage(self.processes(), &jetd, &directory, &home).await?;
        if staged != version {
            return Err(codes::payload_invalid());
        }
        core_cli::activate(self.processes(), &jetd, version, &home).await?;
        Ok(())
    }

    /// Writes the unit when it differs and reloads systemd. `start`: also
    /// enable and start it now (an install); otherwise only enable it.
    async fn register_unit(&mut self, start: bool) -> Result<(), PublicError> {
        let systemctl = self
            .settings()
            .systemctl
            .clone()
            .ok_or_else(codes::systemd_unavailable)?;
        let path = self.settings().unit_path();
        let existed = manager::exists(&path);
        let written = blocking(move || {
            manager::write_if_changed(&path, manager::UNIT_TEMPLATE.as_bytes(), 0o644)
        })
        .await?
        .map_err(|_| codes::install_failed())?;
        let processes = self.processes();
        if written && !manager::run_systemctl(processes, &systemctl, &["daemon-reload"]).await {
            return Err(codes::systemd_unavailable());
        }
        let enable: &[&str] = if start {
            &["enable", "--now", manager::UNIT_NAME]
        } else if !existed {
            &["enable", manager::UNIT_NAME]
        } else {
            return Ok(());
        };
        if manager::run_systemctl(processes, &systemctl, enable).await {
            Ok(())
        } else {
            Err(codes::systemd_unavailable())
        }
    }

    /// An existing unit from an older app gets this build's template.
    async fn refresh_unit(&mut self) -> Result<(), PublicError> {
        if !manager::exists(&self.settings().unit_path()) {
            return Ok(());
        }
        self.register_unit(false).await
    }

    async fn systemd_start(&mut self) -> Result<(), PublicError> {
        let _ = self.refresh_unit().await;
        let systemctl = self
            .settings()
            .systemctl
            .clone()
            .ok_or_else(codes::systemd_unavailable)?;
        let processes = self.processes();
        // A unit that hit its start limit refuses `start` until reset.
        let _ =
            manager::run_systemctl(processes, &systemctl, &["reset-failed", manager::UNIT_NAME])
                .await;
        if manager::run_systemctl(processes, &systemctl, &["start", manager::UNIT_NAME]).await {
            Ok(())
        } else {
            Err(codes::systemd_unavailable())
        }
    }

    /// A detached `current/jetd serve --channel gui` for this session.
    fn spawn(&self) -> Result<(), PublicError> {
        let settings = self.settings();
        if !is_file(&settings.current_jetd()) {
            return Err(codes::start_failed());
        }
        self.processes()
            .spawn_detached(&manager::serve(&settings.jet_home, &settings.user_home))
            .map_err(|_| codes::start_failed())
    }

    /// A reviewed `jetd core rollback`, then the daemon `current` names now.
    /// `Err` means `current` did not move.
    async fn rollback(&mut self, review: &RollbackReview) -> Result<Outcome, PublicError> {
        let service = self.service;
        let status = service.rollback_status().await?;
        if status.current.as_deref() != Some(review.current.as_str())
            || status.previous.as_deref() != Some(review.previous.as_str())
        {
            return Err(codes::review_stale());
        }
        let systemd = self.systemd().await && manager::exists(&self.settings().unit_path());
        let jetd = self.settings().current_jetd();
        // Refused, timed out draining, or failed: `current` did not move.
        core_cli::rollback(self.processes(), &jetd, &self.settings().jet_home).await?;
        let restarted = match (systemd, &status.owner) {
            // The drained daemon comes back through `Restart=always`.
            (true, Owner::Held { .. }) => Ok(()),
            (true, Owner::Free) => self.systemd_start().await,
            (false, _) => self.spawn(),
        };
        let error = match restarted {
            Ok(()) => service.wait_until_reachable().await.err(),
            Err(error) => Some(error),
        };
        Ok(Outcome {
            action: Some(Action::RolledBack),
            error,
            ..Outcome::default()
        })
    }
}

/// Runs short filesystem work off the async workers.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, PublicError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| PublicError::internal())
}

/// Starts the launch work once, off the setup hook: the AppImage launcher
/// entry, one provisioning pass, then the automatic update check.
pub(crate) fn launch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh_launcher(&app).await;
        if let Some(service) = app.try_state::<LocalServiceState>() {
            let service = service.inner().clone();
            service.provision().await;
        }
        super::updates::after_launch(&app).await;
    });
}

async fn refresh_launcher(app: &AppHandle) {
    use tauri::utils::{config::BundleType, platform::bundle_type};
    // The bundler marks the binary; `$APPIMAGE` alone is not proof.
    if bundle_type() != Some(BundleType::AppImage) {
        return;
    }
    let Ok(data_home) = app.path().data_dir() else {
        return;
    };
    let appimage = app.env().appimage;
    let _ = blocking(move || launcher::refresh(appimage.as_deref(), &data_home)).await;
}

#[tauri::command]
pub(crate) async fn load_local_service(
    state: State<'_, LocalServiceState>,
) -> Result<LocalServiceView, PublicError> {
    Ok(state.view())
}

/// One watcher per window; a second call from the same window replaces it.
#[tauri::command]
pub(crate) async fn watch_local_service(
    window: tauri::Window,
    state: State<'_, LocalServiceState>,
    on_change: IpcChannel<LocalServiceView>,
) -> Result<LocalServiceView, PublicError> {
    Ok(state.watch(window.label(), move |view| on_change.send(view).is_ok()))
}

/// Runs the decision table again now.
#[tauri::command]
pub(crate) async fn repair_local_service(
    state: State<'_, LocalServiceState>,
) -> Result<LocalServiceView, PublicError> {
    Ok(state.provision().await)
}

#[tauri::command]
pub(crate) async fn prepare_local_service_rollback(
    state: State<'_, LocalServiceState>,
) -> Result<RollbackReviewView, PublicError> {
    state.prepare_rollback().await
}

#[tauri::command]
pub(crate) async fn execute_local_service_rollback(
    state: State<'_, LocalServiceState>,
    review_id: String,
) -> Result<LocalServiceView, PublicError> {
    if review_id.len() > 64 {
        return Err(codes::review_expired());
    }
    state.execute_rollback(&review_id).await
}
