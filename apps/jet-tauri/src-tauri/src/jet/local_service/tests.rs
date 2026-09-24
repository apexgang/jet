//! Provisioning passes against a simulated computer: `jetd core`, `tar`,
//! `systemctl` and `brew` are played by one scripted handler over a
//! scratch Jet home, the socket by `FakeReachability`, time by `FakeClock`.
//! Nothing runs, listens or sleeps on the test host.
use std::{
    fs, io,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::json;

use super::{
    decision::{Action, Manager, Phase},
    manager,
    payload::{tests::write_payload, BundledPayload},
    process::{
        fake::{FakeClock, FakeProcesses, FakeReachability},
        Finished, Invocation,
    },
    Channel, LocalServiceState, LocalServiceView, ServiceSettings, REVIEW_LIFETIME,
};

const TARGET: &str = super::payload::tests::TARGET;
const BUNDLED: &str = "0.2.0";

#[derive(Debug, Clone, PartialEq)]
enum Held {
    Free,
    Gui(String),
    Homebrew(String),
    Development,
}

/// The simulated computer.
struct Core {
    jet_home: PathBuf,
    current: Option<String>,
    previous: Option<String>,
    owner: Held,
    /// `systemctl --user show-environment` succeeds.
    systemd: bool,
    /// Every other `systemctl` call succeeds.
    systemctl_ok: bool,
    /// The unit is registered, so systemd restarts a drained daemon.
    managed_by_systemd: bool,
    /// A started daemon answers on its socket.
    comes_up: bool,
    /// What the bundled archive unpacks to.
    payload_version: String,
    /// Forces `activate`'s exit code and output.
    activate: Option<(i32, &'static str)>,
    /// `tar` exits 2 (a damaged archive, a noexec cache).
    tar_fails: bool,
    brew_ok: bool,
}

impl Core {
    fn new(jet_home: PathBuf) -> Self {
        Self {
            jet_home,
            current: None,
            previous: None,
            owner: Held::Free,
            systemd: true,
            systemctl_ok: true,
            managed_by_systemd: false,
            comes_up: true,
            payload_version: BUNDLED.into(),
            activate: None,
            tar_fails: false,
            brew_ok: true,
        }
    }

    fn status(&self) -> String {
        let owner = match &self.owner {
            Held::Free => json!({"state": "free"}),
            Held::Gui(version) => {
                json!({"state": "held", "daemon": {"pid": 9, "version": version, "channel": "gui"}})
            }
            Held::Homebrew(version) => {
                json!({"state": "held", "daemon": {"pid": 9, "version": version, "channel": "homebrew"}})
            }
            Held::Development => {
                json!({"state": "held", "daemon": {"pid": 9, "version": "0.2.0-dev", "channel": "development"}})
            }
        };
        json!({
            "status": "ok",
            "current": self.current,
            "previous": self.previous,
            "staged": [],
            "live_helpers": [],
            "owner": owner,
        })
        .to_string()
    }

    /// `current` and `previous` as `jetd core` lays them out.
    fn switch_to(&mut self, version: &str) {
        let versions = self.jet_home.join("core/versions").join(version);
        fs::create_dir_all(&versions).unwrap();
        fs::write(versions.join("jetd"), b"\x7fELF").unwrap();
        fs::write(
            versions.join("manifest.json"),
            json!({"version": version, "target": TARGET}).to_string(),
        )
        .unwrap();
        self.previous = self.current.replace(version.to_owned());
        self.relink();
    }

    fn relink(&self) {
        for (name, version) in [("current", &self.current), ("previous", &self.previous)] {
            let link = self.jet_home.join("core").join(name);
            let _ = fs::remove_file(&link);
            if let Some(version) = version {
                symlink(format!("versions/{version}"), &link).unwrap();
            }
        }
    }
}

fn exit(code: i32, stdout: impl Into<String>) -> io::Result<Finished> {
    Ok(Finished {
        code: Some(code),
        stdout: stdout.into().into_bytes(),
    })
}

fn args(invocation: &Invocation) -> Vec<String> {
    invocation.argv()[1..].to_vec()
}

fn name(invocation: &Invocation) -> String {
    invocation
        .program
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Plays one command against the simulated computer.
fn play(
    core: &Mutex<Core>,
    socket: &FakeReachability,
    invocation: &Invocation,
) -> io::Result<Finished> {
    let mut core = core.lock().unwrap();
    let args = args(invocation);
    match name(invocation).as_str() {
        "tar" => {
            if core.tar_fails {
                return exit(2, "");
            }
            let directory = &args[args.iter().position(|arg| arg == "-C").unwrap() + 1];
            write_payload(Path::new(directory), &core.payload_version, TARGET);
            exit(0, "")
        }
        "systemctl" => match args[1].as_str() {
            "show-environment" => exit(if core.systemd { 0 } else { 1 }, ""),
            _ if !core.systemctl_ok => exit(1, ""),
            "enable" if args.contains(&"--now".to_owned()) => {
                core.managed_by_systemd = true;
                if let Some(current) = core.current.clone() {
                    if core.comes_up {
                        core.owner = Held::Gui(current);
                        socket.set(true);
                    }
                }
                exit(0, "")
            }
            "start" => {
                if let (Some(current), true) = (core.current.clone(), core.comes_up) {
                    core.owner = Held::Gui(current);
                    socket.set(true);
                }
                exit(0, "")
            }
            _ => exit(0, ""),
        },
        "brew" => {
            assert_eq!(args, ["services", "start", "apexgang/tap/jetd"]);
            if !core.brew_ok {
                return exit(1, "");
            }
            core.owner = Held::Homebrew("0.2.0".into());
            socket.set(true);
            exit(0, "")
        }
        "jetd" => {
            assert_eq!(args[0], "core");
            assert_eq!(args[args.len() - 2], "--home");
            assert_eq!(Path::new(&args[args.len() - 1]), core.jet_home);
            match args[1].as_str() {
                "status" => exit(0, core.status()),
                "stage" => exit(
                    0,
                    json!({"status": "staged", "version": core.payload_version, "target": TARGET})
                        .to_string(),
                ),
                "activate" => {
                    if let Some((code, stdout)) = core.activate {
                        return exit(code, stdout);
                    }
                    let version = args[3].clone();
                    drain_and_switch(&mut core, socket, &version);
                    exit(
                        0,
                        json!({"status": "activated", "version": version}).to_string(),
                    )
                }
                "rollback" => {
                    let Some(previous) = core.previous.clone() else {
                        return exit(3, r#"{"status":"refused","code":"no_previous_version"}"#);
                    };
                    drain_and_switch(&mut core, socket, &previous);
                    exit(
                        0,
                        json!({"status": "activated", "version": previous}).to_string(),
                    )
                }
                other => panic!("unexpected jetd core {other}"),
            }
        }
        other => panic!("unexpected program {other}"),
    }
}

/// Activation drains the owner; systemd brings the new `current` back.
fn drain_and_switch(core: &mut Core, socket: &FakeReachability, version: &str) {
    let was_held = core.owner != Held::Free;
    core.switch_to(version);
    core.owner = Held::Free;
    socket.set(false);
    if was_held && core.managed_by_systemd && core.comes_up {
        core.owner = Held::Gui(version.to_owned());
        socket.set(true);
    }
}

struct World {
    _root: tempfile::TempDir,
    settings: ServiceSettings,
    core: Arc<Mutex<Core>>,
    processes: Arc<FakeProcesses>,
    socket: Arc<FakeReachability>,
    clock: Arc<FakeClock>,
    state: LocalServiceState,
}

struct Setup {
    bundled: bool,
    keg: bool,
    configure: fn(&mut Core),
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            bundled: true,
            keg: false,
            configure: |_| (),
        }
    }
}

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn world(setup: Setup) -> World {
    let root = tempfile::tempdir().unwrap();
    let base = root.path();
    let jet_home = base.join("home/.jet");
    fs::create_dir_all(&jet_home).unwrap();
    let archive = base.join("resources/jet-core.tar.gz");
    if setup.bundled {
        fs::create_dir_all(archive.parent().unwrap()).unwrap();
        fs::write(&archive, b"gzip").unwrap();
    }
    let prefix = base.join("linuxbrew");
    if setup.keg {
        executable(&prefix.join("opt/jetd/bin/jetd"));
        executable(&prefix.join("bin/brew"));
    }
    let app_data = base.join("app-data");
    fs::create_dir_all(&app_data).unwrap();
    let settings = ServiceSettings {
        jet_home: jet_home.clone(),
        user_home: base.join("home"),
        config_home: base.join("home/.config"),
        scratch: base.join("cache"),
        lock_file: app_data.join(super::LOCK_FILE),
        bundled: Some(BundledPayload {
            archive,
            version: BUNDLED.into(),
            target: TARGET.into(),
        }),
        homebrew_prefixes: vec![prefix],
        owner_uid: std::os::unix::fs::MetadataExt::uid(&fs::metadata(base).unwrap()),
        systemctl: Some(PathBuf::from("/usr/bin/systemctl")),
        tar: Some(PathBuf::from("/usr/bin/tar")),
    };
    let mut core = Core::new(jet_home);
    (setup.configure)(&mut core);
    core.relink();
    let reachable = core.owner != Held::Free;
    let core = Arc::new(Mutex::new(core));
    let socket = Arc::new(FakeReachability::new(reachable));
    let processes = {
        let (core, socket) = (core.clone(), socket.clone());
        let spawned = (core.clone(), socket.clone());
        Arc::new(
            FakeProcesses::new(move |invocation| play(&core, &socket, invocation)).on_spawn(
                move |_| {
                    let mut core = spawned.0.lock().unwrap();
                    if let (Some(current), true) = (core.current.clone(), core.comes_up) {
                        core.owner = Held::Gui(current);
                        spawned.1.set(true);
                    }
                },
            ),
        )
    };
    let clock = Arc::new(FakeClock::new());
    let state = LocalServiceState::new(
        settings.clone(),
        processes.clone(),
        socket.clone(),
        clock.clone(),
    );
    World {
        _root: root,
        settings,
        core,
        processes,
        socket,
        clock,
        state,
    }
}

impl World {
    /// The commands run, without the program's directory.
    fn commands(&self) -> Vec<String> {
        self.processes
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|invocation| {
                let mut words = vec![name(invocation)];
                words.extend(
                    args(invocation)
                        .into_iter()
                        .filter(|arg| !arg.starts_with('/')),
                );
                words.join(" ")
            })
            .collect()
    }

    fn ran(&self, command: &str) -> bool {
        self.commands().iter().any(|candidate| candidate == command)
    }

    fn extractions_left(&self) -> usize {
        fs::read_dir(self.settings.scratch.join("local-service"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    fn unit(&self) -> PathBuf {
        manager::unit_path(&self.settings.config_home)
    }

    fn autostart(&self) -> PathBuf {
        manager::autostart_path(&self.settings.config_home)
    }
}

fn installed(core: &mut Core, version: &str) {
    core.switch_to(version);
}

fn code(view: &LocalServiceView) -> Option<&str> {
    view.error.as_ref().map(|error| error.code.as_str())
}

#[tokio::test]
async fn a_fresh_computer_gets_the_bundled_service_under_systemd() {
    let world = world(Setup::default());
    assert_eq!(world.state.view().phase, Phase::Checking);
    assert_eq!(world.state.view().bundled_version.as_deref(), Some(BUNDLED));

    let view = world.state.provision().await;

    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(view.channel, Some(Channel::Gui));
    assert_eq!(view.manager, Some(Manager::Systemd));
    assert_eq!(view.current_version.as_deref(), Some(BUNDLED));
    assert_eq!(view.running_version.as_deref(), Some(BUNDLED));
    assert_eq!(view.last_action, Some(Action::Installed));
    assert_eq!(view.error, None);
    assert!(!view.can_repair && !view.can_rollback);
    assert_eq!(
        world.commands(),
        [
            "tar --no-same-owner --no-same-permissions -xzf -C",
            "jetd core status --home",
            "systemctl --user show-environment",
            "jetd core stage --payload --home",
            "jetd core activate --version 0.2.0 --home",
            "systemctl --user daemon-reload",
            "systemctl --user enable --now jetd.service",
            "jetd core status --home",
        ]
    );
    // Staged from the unpacked archive, by the payload's own jetd.
    let calls = world.processes.argvs();
    assert!(calls[3][0].contains("/cache/local-service/payload-"));
    assert!(calls[3][4].ends_with(&format!("jet-core-{BUNDLED}-{TARGET}")));
    // The re-observation uses the activated core.
    assert!(calls[7][0].ends_with(".jet/core/current/jetd"));
    assert_eq!(
        fs::read_to_string(world.unit()).unwrap(),
        manager::UNIT_TEMPLATE
    );
    assert!(!world.autostart().exists());
    assert_eq!(
        world.extractions_left(),
        0,
        "the unpacked payload is removed"
    );
    assert!(world.processes.detached_argvs().is_empty());
}

#[tokio::test]
async fn without_systemd_the_app_writes_autostart_and_starts_jetd_itself() {
    let world = world(Setup {
        configure: |core| core.systemd = false,
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(view.manager, Some(Manager::Autostart));
    assert_eq!(view.last_action, Some(Action::Installed));
    assert_eq!(
        fs::read_to_string(world.autostart()).unwrap(),
        manager::AUTOSTART_TEMPLATE
    );
    assert!(!world.unit().exists());
    assert!(!world.ran("systemctl --user enable --now jetd.service"));
    let detached = world.processes.detached.lock().unwrap().clone();
    assert_eq!(detached.len(), 1);
    assert_eq!(args(&detached[0]), ["serve", "--channel", "gui"]);
    assert!(detached[0].program.ends_with(".jet/core/current/jetd"));
    let path = detached[0].path_env.clone().unwrap();
    assert!(path
        .to_string_lossy()
        .ends_with("/home/.local/bin:/usr/local/bin:/usr/bin:/bin"));
}

#[tokio::test]
async fn a_homebrew_daemon_is_never_staged_or_activated() {
    let world = world(Setup {
        keg: true,
        configure: |core| core.owner = Held::Homebrew("0.1.0".into()),
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running);
    assert_eq!(view.channel, Some(Channel::Homebrew));
    assert_eq!(view.manager, Some(Manager::BrewServices));
    assert_eq!(view.running_version.as_deref(), Some("0.1.0"));
    assert!(!view.can_rollback);
    // The keg's jetd reports; nothing is unpacked, staged or registered.
    assert_eq!(
        world.commands(),
        [
            "jetd core status --home",
            "systemctl --user show-environment",
            "jetd core status --home",
        ]
    );
    assert!(world.processes.argvs()[0][0].contains("linuxbrew"));
    assert!(!world.unit().exists());
}

#[tokio::test]
async fn a_development_daemon_is_left_alone() {
    let world = world(Setup {
        configure: |core| core.owner = Held::Development,
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running);
    assert_eq!(view.channel, Some(Channel::Development));
    assert_eq!(view.manager, None);
    assert!(!world.ran("jetd core stage --payload --home"));
    assert!(!world.ran("jetd core activate --version 0.2.0 --home"));
}

#[tokio::test]
async fn an_older_gui_daemon_is_updated_and_systemd_restarts_it() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.1.0");
            core.owner = Held::Gui("0.1.0".into());
            core.managed_by_systemd = true;
        },
        ..Setup::default()
    });
    manager::write_if_changed(&world.unit(), manager::UNIT_TEMPLATE.as_bytes(), 0o644).unwrap();

    let view = world.state.provision().await;

    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(view.last_action, Some(Action::Updated));
    assert_eq!(view.current_version.as_deref(), Some("0.2.0"));
    assert_eq!(view.previous_version.as_deref(), Some("0.1.0"));
    assert_eq!(view.running_version.as_deref(), Some("0.2.0"));
    assert!(view.can_rollback);
    assert!(world.ran("jetd core activate --version 0.2.0 --home"));
    // systemd restarts it: no `start`, no detached daemon, and the unit
    // (already current) is not reloaded.
    assert!(!world.ran("systemctl --user start jetd.service"));
    assert!(!world.ran("systemctl --user daemon-reload"));
    assert!(world.processes.detached_argvs().is_empty());
}

#[tokio::test]
async fn a_current_gui_daemon_is_never_restarted() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.2.0");
            core.owner = Held::Gui("0.2.0".into());
        },
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running);
    assert_eq!(view.last_action, None);
    // The unit is registered for later logins, never started now.
    assert_eq!(
        fs::read_to_string(world.unit()).unwrap(),
        manager::UNIT_TEMPLATE
    );
    assert!(world.ran("systemctl --user daemon-reload"));
    assert!(world.ran("systemctl --user enable jetd.service"));
    assert!(!world.ran("systemctl --user enable --now jetd.service"));
    assert!(!world.ran("systemctl --user start jetd.service"));
    assert!(!world.ran("jetd core stage --payload --home"));
}

#[tokio::test]
async fn a_stopped_gui_service_is_started() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.2.0");
            core.managed_by_systemd = true;
        },
        ..Setup::default()
    });
    manager::write_if_changed(&world.unit(), manager::UNIT_TEMPLATE.as_bytes(), 0o644).unwrap();
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(view.last_action, Some(Action::Started));
    assert!(world.ran("systemctl --user reset-failed jetd.service"));
    assert!(world.ran("systemctl --user start jetd.service"));
    // Its own jetd reports; the bundled archive is never unpacked.
    assert!(!world.ran("tar --no-same-owner --no-same-permissions -xzf -C"));
}

#[tokio::test]
async fn a_stopped_homebrew_keg_is_started_with_brew_services() {
    let world = world(Setup {
        keg: true,
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(view.channel, Some(Channel::Homebrew));
    assert_eq!(view.last_action, Some(Action::Started));
    assert!(world.ran("brew services start apexgang/tap/jetd"));
    assert!(!world.ran("jetd core stage --payload --home"));
    assert!(world
        .processes
        .argvs()
        .iter()
        .any(|argv| argv[0].ends_with("linuxbrew/bin/brew")));

    let failing = world_with_brew_failure().await;
    assert_eq!(failing.phase, Phase::Failed);
    assert_eq!(code(&failing), Some("service.homebrew_start_failed"));
    assert!(failing.can_repair);
}

async fn world_with_brew_failure() -> LocalServiceView {
    let world = world(Setup {
        keg: true,
        configure: |core| core.brew_ok = false,
        ..Setup::default()
    });
    world.state.provision().await
}

#[tokio::test]
async fn a_development_build_without_anything_reports_not_installed() {
    let world = world(Setup {
        bundled: false,
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::NotInstalled);
    assert_eq!(view.bundled_version, None);
    assert!(!view.can_repair);
    assert_eq!(world.commands(), ["systemctl --user show-environment"]);
}

#[tokio::test]
async fn a_daemon_that_never_answers_times_out_without_sleeping() {
    let world = world(Setup {
        configure: |core| core.comes_up = false,
        ..Setup::default()
    });
    let started = std::time::Instant::now();
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Failed);
    assert_eq!(code(&view), Some("service.start_timeout"));
    assert!(view.can_repair);
    assert!(world.clock.slept() >= Duration::from_secs(15));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn refusals_from_jetd_core_become_stable_codes() {
    let cases: [(&str, (i32, &'static str)); 3] = [
        (
            "service.channel_owned",
            (
                3,
                r#"{"status":"refused","code":"channel_owned","owner":{"pid":1,"version":"0.2.0","channel":"homebrew"}}"#,
            ),
        ),
        (
            "service.drain_timeout",
            (4, r#"{"status":"drain_timeout"}"#),
        ),
        ("service.install_failed", (1, "")),
    ];
    for (expected, answer) in cases {
        let world = world(Setup::default());
        world.core.lock().unwrap().activate = Some(answer);
        let view = world.state.provision().await;
        assert_eq!(view.phase, Phase::Failed, "{expected}");
        assert_eq!(code(&view), Some(expected));
        assert!(!world.unit().exists(), "{expected}: nothing registered");
        assert_eq!(world.extractions_left(), 0);
    }
}

/// A stopped GUI service whose bundled update fails is started as it is,
/// and the view keeps why it was not updated.
#[tokio::test]
async fn a_failed_update_still_starts_the_stopped_service() {
    type Failure = fn(&mut Core);
    let cases: [(&str, Failure); 3] = [
        ("service.update_refused", |core| {
            // Helpers of the stopped daemon still speak its protocol major.
            core.activate = Some((3, r#"{"status":"refused","code":"protocol_major_pinned"}"#));
        }),
        ("service.install_failed", |core| {
            core.activate = Some((1, ""))
        }),
        ("service.payload_invalid", |core| core.tar_fails = true),
    ];
    for (expected, fail) in cases {
        let world = world(Setup {
            configure: |core| {
                installed(core, "0.1.0");
                core.managed_by_systemd = true;
            },
            ..Setup::default()
        });
        fail(&mut world.core.lock().unwrap());
        manager::write_if_changed(&world.unit(), manager::UNIT_TEMPLATE.as_bytes(), 0o644).unwrap();

        let view = world.state.provision().await;

        assert_eq!(view.phase, Phase::Running, "{expected}: {view:?}");
        assert_eq!(code(&view), Some(expected));
        assert_eq!(view.current_version.as_deref(), Some("0.1.0"), "{expected}");
        assert_eq!(view.running_version.as_deref(), Some("0.1.0"), "{expected}");
        assert_eq!(view.last_action, None, "{expected}");
        assert!(
            world.ran("systemctl --user start jetd.service"),
            "{expected}: {:?}",
            world.commands()
        );
        assert_eq!(world.extractions_left(), 0);
    }
}

/// An older `current` without a service file is registered and started
/// even when the bundled update is refused.
#[tokio::test]
async fn an_older_current_is_registered_when_its_update_is_refused() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.1.0");
            core.activate = Some((3, r#"{"status":"refused","code":"schema_outside_pair"}"#));
        },
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(code(&view), Some("service.update_refused"));
    assert_eq!(view.current_version.as_deref(), Some("0.1.0"));
    assert_eq!(view.manager, Some(Manager::Systemd));
    assert!(world.ran("systemctl --user enable --now jetd.service"));
    assert_eq!(
        fs::read_to_string(world.unit()).unwrap(),
        manager::UNIT_TEMPLATE
    );
}

/// A daemon of another channel took the Plane during the update: a second
/// daemon would only compete for its lock, so nothing is started.
#[tokio::test]
async fn nothing_starts_when_another_daemon_took_the_plane() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.1.0");
            core.activate = Some((
                3,
                r#"{"status":"refused","code":"channel_owned","owner":{"pid":1,"version":"0.2.0","channel":"homebrew"}}"#,
            ));
        },
        ..Setup::default()
    });
    manager::write_if_changed(&world.unit(), manager::UNIT_TEMPLATE.as_bytes(), 0o644).unwrap();
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Failed, "{view:?}");
    assert_eq!(code(&view), Some("service.channel_owned"));
    assert!(!world.ran("systemctl --user start jetd.service"));
}

/// A daemon that accepts but never answers (stopped with SIGSTOP, wedged)
/// ends the pass within its deadlines, so the next pass is not blocked.
#[tokio::test]
async fn a_socket_that_never_answers_ends_the_pass() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.2.0");
            core.owner = Held::Gui("0.2.0".into());
        },
        ..Setup::default()
    });
    world.socket.hang(true);
    // A real-time guard only: the pass runs on the fake clock.
    let view = tokio::time::timeout(Duration::from_secs(10), world.state.provision())
        .await
        .expect("the pass ends");
    assert_eq!(view.phase, Phase::Failed, "{view:?}");
    assert_eq!(code(&view), Some("service.start_timeout"));
    assert!(view.can_repair);

    world.socket.hang(false);
    let next = tokio::time::timeout(Duration::from_secs(10), world.state.provision())
        .await
        .expect("a later pass is not blocked");
    assert_eq!(next.phase, Phase::Running, "{next:?}");
    assert_eq!(next.error, None);
}

#[tokio::test]
async fn a_payload_for_another_version_is_refused_before_staging() {
    let world = world(Setup {
        configure: |core| core.payload_version = "0.1.0".into(),
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Failed);
    assert_eq!(code(&view), Some("service.payload_invalid"));
    assert!(!world.ran("jetd core stage --payload --home"));
    assert_eq!(world.extractions_left(), 0);
}

#[tokio::test]
async fn a_systemd_refusal_is_reported_and_repair_retries() {
    let world = world(Setup {
        configure: |core| core.systemctl_ok = false,
        ..Setup::default()
    });
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Failed);
    assert_eq!(code(&view), Some("service.systemd_unavailable"));
    assert!(view.can_repair);

    world.core.lock().unwrap().systemctl_ok = true;
    let repaired = world.state.provision().await;
    assert_eq!(repaired.phase, Phase::Running, "{repaired:?}");
    // `current` exists and the unit was written: the repair starts it.
    assert_eq!(repaired.last_action, Some(Action::Started));
}

#[tokio::test]
async fn reviewed_rollback_switches_back_and_sticks() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.1.0");
            installed(core, "0.2.0");
            core.owner = Held::Gui("0.2.0".into());
            core.managed_by_systemd = true;
        },
        ..Setup::default()
    });
    manager::write_if_changed(&world.unit(), manager::UNIT_TEMPLATE.as_bytes(), 0o644).unwrap();
    assert!(world.state.provision().await.can_rollback);

    let review = world.state.prepare_rollback().await.unwrap();
    let json = serde_json::to_value(&review).unwrap();
    assert_eq!(json["currentVersion"], "0.2.0");
    assert_eq!(json["previousVersion"], "0.1.0");
    let review_id = json["reviewId"].as_str().unwrap().to_owned();

    let view = world.state.execute_rollback(&review_id).await.unwrap();
    assert_eq!(view.phase, Phase::Running, "{view:?}");
    assert_eq!(view.last_action, Some(Action::RolledBack));
    assert_eq!(view.current_version.as_deref(), Some("0.1.0"));
    assert_eq!(view.previous_version.as_deref(), Some("0.2.0"));
    assert!(world.ran("jetd core rollback --home"));

    // Used once.
    assert_eq!(
        world
            .state
            .execute_rollback(&review_id)
            .await
            .unwrap_err()
            .code,
        "service.review_expired"
    );
    // The bundled 0.2.0 is what the rollback left: the next launch keeps 0.1.0.
    let next = world.state.provision().await;
    assert_eq!(next.current_version.as_deref(), Some("0.1.0"));
    assert_eq!(next.last_action, None);
    assert!(!world.ran("jetd core activate --version 0.2.0 --home"));
}

#[tokio::test]
async fn rollback_reviews_expire_go_stale_and_respect_other_channels() {
    let world = world(Setup {
        configure: |core| {
            installed(core, "0.1.0");
            installed(core, "0.2.0");
            core.owner = Held::Gui("0.2.0".into());
        },
        ..Setup::default()
    });
    assert_eq!(
        world
            .state
            .execute_rollback("not-a-uuid")
            .await
            .unwrap_err()
            .code,
        "service.review_expired"
    );

    let first = world.state.prepare_rollback().await.unwrap();
    let first_id = serde_json::to_value(&first).unwrap()["reviewId"]
        .as_str()
        .unwrap()
        .to_owned();
    world
        .clock
        .advance(REVIEW_LIFETIME + Duration::from_secs(1));
    assert_eq!(
        world
            .state
            .execute_rollback(&first_id)
            .await
            .unwrap_err()
            .code,
        "service.review_expired"
    );

    let second = world.state.prepare_rollback().await.unwrap();
    let second_id = serde_json::to_value(&second).unwrap()["reviewId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Another app instance moved `current` in between.
    world.core.lock().unwrap().switch_to("0.3.0");
    assert_eq!(
        world
            .state
            .execute_rollback(&second_id)
            .await
            .unwrap_err()
            .code,
        "service.review_stale"
    );
    assert!(!world.ran("jetd core rollback --home"));

    world.core.lock().unwrap().owner = Held::Homebrew("0.2.0".into());
    assert_eq!(
        world.state.prepare_rollback().await.unwrap_err().code,
        "service.channel_owned"
    );

    let nothing = self::world(Setup::default());
    assert_eq!(
        nothing.state.prepare_rollback().await.unwrap_err().code,
        "service.rollback_unavailable"
    );
}

#[tokio::test]
async fn another_instance_holding_the_lock_keeps_this_one_out() {
    let world = world(Setup::default());
    let other = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&world.settings.lock_file)
        .unwrap();
    other.lock().unwrap();
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Failed);
    assert_eq!(code(&view), Some("service.busy"));
    assert!(world.processes.calls.lock().unwrap().is_empty());
    assert!(world.clock.slept() >= Duration::from_secs(60));

    other.unlock().unwrap();
    assert_eq!(world.state.provision().await.phase, Phase::Running);
}

/// A directory, or any unopenable file, in place of `local-service.lock`
/// fails the pass without a panic and without running anything.
#[tokio::test]
async fn an_unusable_lock_file_fails_the_pass_safely() {
    let world = world(Setup::default());
    fs::create_dir_all(&world.settings.lock_file).unwrap();
    let view = world.state.provision().await;
    assert_eq!(view.phase, Phase::Failed);
    assert_eq!(code(&view), Some("service.lock_unavailable"));
    assert!(world.processes.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn watchers_see_each_phase_and_stop_with_their_window() {
    let world = world(Setup::default());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let initial = world.state.watch("main", move |view| {
        sink.lock().unwrap().push(view.phase);
        true
    });
    assert_eq!(initial.phase, Phase::Checking);
    let replaced = Arc::new(Mutex::new(Vec::new()));
    let stale = replaced.clone();
    // Same window again: the new watcher replaces the old one.
    world.state.watch("settings", move |view| {
        stale.lock().unwrap().push(view.phase);
        true
    });
    world.state.window_destroyed("settings");

    world.state.provision().await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let phases = seen.lock().unwrap().clone();
    assert!(phases.contains(&Phase::Installing), "{phases:?}");
    assert_eq!(phases.last(), Some(&Phase::Running), "{phases:?}");
    assert!(replaced.lock().unwrap().is_empty());
    assert!(
        world
            .socket
            .probes
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    );
}
