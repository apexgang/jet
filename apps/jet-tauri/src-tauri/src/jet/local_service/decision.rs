//! The provisioning decision table (Wave 4 §A) as a pure function over
//! observed facts, and the view a finished pass reports.
//!
//! | Owner | Other facts | Plan |
//! | --- | --- | --- |
//! | held, `homebrew` | – | keep; Homebrew manages it, the app updater is off |
//! | held, `development` | – | keep |
//! | held, `gui` | bundled newer than `current` | stage, activate; the manager restarts `jetd` |
//! | held, `gui` | otherwise | keep; ensure the unit when systemd manages it |
//! | held, unknown metadata | – | keep; channel unknown |
//! | free | `current` and a GUI unit or autostart file | update first when bundled is newer, then start |
//! | free | Homebrew keg | `brew services start` |
//! | free | bundled payload | install: stage, activate, register, start |
//! | free | nothing | not installed |
//!
//! "Newer" is a semver comparison for the same target, and never the
//! version `previous` names: that is the release a reviewed rollback left,
//! and a bundled copy of it must not undo the rollback on the next launch.
//! `current` is never downgraded, and nothing is staged or activated for
//! another channel or when a Homebrew keg is chosen. When the update before
//! a start (or before registering an older `current`) fails, the `current`
//! it left is still started and the view keeps the failure, unless another
//! daemon took the Plane meanwhile (`mod.rs`, `try_update`).
use semver::Version;
use serde::Serialize;

use super::core_cli::Channel;
use crate::jet::errors::PublicError;

/// The Plane's owner as `jetd core status` saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnerFact {
    Free,
    /// `None`: the lock metadata is missing or unknown.
    Held(Option<Channel>),
    /// No `jetd` could report: nothing is installed, or status failed.
    Unknown,
}

/// A release version and the target it was built for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Release {
    pub(crate) version: String,
    /// `None` when `current`'s manifest could not be read.
    pub(crate) target: Option<String>,
}

/// Everything the table reads; gathered natively before each decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) owner: OwnerFact,
    /// The version the running daemon reports, when held.
    pub(crate) running: Option<String>,
    pub(crate) current: Option<Release>,
    pub(crate) previous: Option<String>,
    /// `$XDG_CONFIG_HOME/systemd/user/jetd.service` exists.
    pub(crate) unit_file: bool,
    /// `$XDG_CONFIG_HOME/autostart/jetd.desktop` exists.
    pub(crate) autostart_file: bool,
    /// `systemctl --user show-environment` succeeded.
    pub(crate) systemd: bool,
    /// A trusted Homebrew `jet` keg's `jetd` and `brew`.
    pub(crate) keg: bool,
    /// The payload this build carries.
    pub(crate) bundled: Option<Release>,
    pub(crate) reachable: bool,
}

/// How a started GUI daemon is (re)started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Via {
    /// `systemctl --user`; `Restart=always` brings a drained daemon back.
    Systemd,
    /// A detached `current/jetd serve --channel gui`, with the XDG
    /// autostart file for later logins. Nothing restarts it.
    Spawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Leave the daemon alone. `ensure_unit`: write and enable (never
    /// start) the GUI unit for later logins.
    Keep {
        channel: Option<Channel>,
        ensure_unit: bool,
    },
    /// A GUI daemon runs an older release: stage and activate `version`.
    /// Activation drains it; `via` brings the new one up.
    Update {
        version: String,
        via: Via,
    },
    /// A stopped GUI installation: optionally update, then start it.
    Start {
        update: Option<String>,
        via: Via,
    },
    /// A stopped Homebrew installation: `brew services start`.
    BrewStart,
    /// Install the bundled payload. `activate` is `None` when `current` is
    /// already as new: it is never downgraded.
    Install {
        activate: Option<String>,
        via: Via,
    },
    NotInstalled,
}

impl Plan {
    /// The channel the plan puts in charge, when the owner does not say.
    pub(crate) fn channel(&self) -> Option<Channel> {
        match self {
            Self::Keep { channel, .. } => *channel,
            Self::Update { .. } | Self::Start { .. } | Self::Install { .. } => Some(Channel::Gui),
            Self::BrewStart => Some(Channel::Homebrew),
            Self::NotInstalled => None,
        }
    }
}

/// The bundled version, when it should replace `current`.
pub(crate) fn upgrade(facts: &Facts) -> Option<String> {
    let bundled = facts.bundled.as_ref()?;
    let current = facts.current.as_ref()?;
    if facts.previous.as_deref() == Some(bundled.version.as_str()) {
        return None;
    }
    if bundled.target.is_none() || current.target != bundled.target {
        return None;
    }
    let newer = match (
        Version::parse(&bundled.version),
        Version::parse(&current.version),
    ) {
        (Ok(bundled), Ok(current)) => bundled > current,
        _ => false,
    };
    newer.then(|| bundled.version.clone())
}

pub(crate) fn decide(facts: &Facts) -> Plan {
    let manager = if facts.systemd && facts.unit_file {
        Via::Systemd
    } else {
        Via::Spawn
    };
    match &facts.owner {
        OwnerFact::Held(Some(Channel::Gui)) => match upgrade(facts) {
            Some(version) => Plan::Update {
                version,
                via: manager,
            },
            None => Plan::Keep {
                channel: Some(Channel::Gui),
                // An autostart-managed daemon keeps its autostart file: two
                // managers would race for the lock at the next login.
                ensure_unit: facts.systemd && !facts.autostart_file,
            },
        },
        OwnerFact::Held(channel) => Plan::Keep {
            channel: *channel,
            ensure_unit: false,
        },
        OwnerFact::Unknown if facts.reachable => Plan::Keep {
            channel: None,
            ensure_unit: false,
        },
        OwnerFact::Free | OwnerFact::Unknown => {
            if facts.current.is_some() && (facts.unit_file || facts.autostart_file) {
                Plan::Start {
                    update: upgrade(facts),
                    via: manager,
                }
            } else if facts.keg {
                Plan::BrewStart
            } else if let Some(bundled) = &facts.bundled {
                let activate = match &facts.current {
                    None => Some(bundled.version.clone()),
                    Some(_) => upgrade(facts),
                };
                Plan::Install {
                    activate,
                    via: if facts.systemd {
                        Via::Systemd
                    } else {
                        Via::Spawn
                    },
                }
            } else {
                Plan::NotInstalled
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The view a pass reports
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Checking,
    Installing,
    Updating,
    Starting,
    Running,
    Stopped,
    NotInstalled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Manager {
    Systemd,
    Autostart,
    BrewServices,
}

/// What the last pass changed, for a one-line confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Installed,
    Updated,
    Started,
    RolledBack,
}

/// The local service as the webview sees it. No path, pid or native text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalServiceView {
    pub(crate) phase: Phase,
    pub(crate) channel: Option<Channel>,
    pub(crate) manager: Option<Manager>,
    pub(crate) bundled_version: Option<String>,
    pub(crate) current_version: Option<String>,
    pub(crate) previous_version: Option<String>,
    pub(crate) running_version: Option<String>,
    pub(crate) can_repair: bool,
    pub(crate) can_rollback: bool,
    pub(crate) last_action: Option<Action>,
    pub(crate) error: Option<PublicError>,
}

impl LocalServiceView {
    /// Before the first observation.
    pub(crate) fn checking(bundled_version: Option<String>) -> Self {
        Self {
            phase: Phase::Checking,
            channel: None,
            manager: None,
            bundled_version,
            current_version: None,
            previous_version: None,
            running_version: None,
            can_repair: false,
            can_rollback: false,
            last_action: None,
            error: None,
        }
    }

    /// The same facts while a pass works: nothing can be started twice.
    pub(crate) fn working(&self, phase: Phase) -> Self {
        Self {
            phase,
            can_repair: false,
            can_rollback: false,
            last_action: None,
            error: None,
            ..self.clone()
        }
    }
}

/// What a pass did, beside the facts it re-observed afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// The channel the plan chose, used when the owner does not name one.
    pub(crate) channel: Option<Channel>,
    pub(crate) action: Option<Action>,
    pub(crate) error: Option<PublicError>,
    pub(crate) not_installed: bool,
}

/// The view of re-observed `facts` after a pass with `outcome`.
pub(crate) fn settle(facts: &Facts, outcome: &Outcome) -> LocalServiceView {
    let phase = if facts.reachable {
        Phase::Running
    } else if outcome.error.is_some() {
        Phase::Failed
    } else if outcome.not_installed {
        Phase::NotInstalled
    } else {
        Phase::Stopped
    };
    let channel = match &facts.owner {
        OwnerFact::Held(channel) => *channel,
        OwnerFact::Free | OwnerFact::Unknown => outcome.channel,
    };
    let manager = match channel {
        Some(Channel::Homebrew) => Some(Manager::BrewServices),
        Some(Channel::Gui) if facts.systemd && facts.unit_file => Some(Manager::Systemd),
        Some(Channel::Gui) if facts.autostart_file => Some(Manager::Autostart),
        _ => None,
    };
    LocalServiceView {
        phase,
        channel,
        manager,
        bundled_version: facts
            .bundled
            .as_ref()
            .map(|bundled| bundled.version.clone()),
        current_version: facts
            .current
            .as_ref()
            .map(|current| current.version.clone()),
        previous_version: facts.previous.clone(),
        running_version: facts.running.clone(),
        can_repair: phase != Phase::Running
            && (facts.bundled.is_some() || facts.keg || facts.current.is_some()),
        can_rollback: channel == Some(Channel::Gui)
            && facts.current.is_some()
            && facts.previous.is_some(),
        last_action: outcome.action.filter(|_| outcome.error.is_none()),
        // Kept while running too: an update that failed leaves the old
        // daemon serving, and the user still learns why.
        error: outcome.error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn release(version: &str) -> Option<Release> {
        Some(Release {
            version: version.into(),
            target: Some(TARGET.into()),
        })
    }

    fn free() -> Facts {
        Facts {
            owner: OwnerFact::Free,
            running: None,
            current: None,
            previous: None,
            unit_file: false,
            autostart_file: false,
            systemd: true,
            keg: false,
            bundled: None,
            reachable: false,
        }
    }

    fn held(channel: Option<Channel>) -> Facts {
        Facts {
            owner: OwnerFact::Held(channel),
            running: Some("0.1.0".into()),
            reachable: true,
            ..free()
        }
    }

    /// Every row of the table, and the guards around it.
    #[test]
    fn decision_table() {
        let gui = Some(Channel::Gui);
        let cases: Vec<(&str, Facts, Plan)> = vec![
            (
                "held homebrew: never staged, even with a newer bundle",
                Facts {
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    keg: true,
                    ..held(Some(Channel::Homebrew))
                },
                Plan::Keep {
                    channel: Some(Channel::Homebrew),
                    ensure_unit: false,
                },
            ),
            (
                "held development",
                Facts {
                    bundled: release("0.2.0"),
                    ..held(Some(Channel::Development))
                },
                Plan::Keep {
                    channel: Some(Channel::Development),
                    ensure_unit: false,
                },
            ),
            (
                "held unknown metadata",
                Facts {
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    ..held(None)
                },
                Plan::Keep {
                    channel: None,
                    ensure_unit: false,
                },
            ),
            (
                "held gui, bundled newer, systemd unit",
                Facts {
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    unit_file: true,
                    ..held(gui)
                },
                Plan::Update {
                    version: "0.2.0".into(),
                    via: Via::Systemd,
                },
            ),
            (
                "held gui, bundled newer, autostart: respawned",
                Facts {
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    autostart_file: true,
                    systemd: false,
                    ..held(gui)
                },
                Plan::Update {
                    version: "0.2.0".into(),
                    via: Via::Spawn,
                },
            ),
            (
                "held gui, same version: keep, unit present",
                Facts {
                    current: release("0.2.0"),
                    bundled: release("0.2.0"),
                    unit_file: true,
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: true,
                },
            ),
            (
                "held gui, bundled older: never downgraded",
                Facts {
                    current: release("0.3.0"),
                    bundled: release("0.2.0"),
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: true,
                },
            ),
            (
                "held gui, bundled is what a rollback left",
                Facts {
                    current: release("0.1.0"),
                    previous: Some("0.2.0".into()),
                    bundled: release("0.2.0"),
                    unit_file: true,
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: true,
                },
            ),
            (
                "held gui, another target",
                Facts {
                    current: Some(Release {
                        version: "0.1.0".into(),
                        target: Some("aarch64-unknown-linux-gnu".into()),
                    }),
                    bundled: release("0.2.0"),
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: true,
                },
            ),
            (
                "held gui, unreadable current manifest",
                Facts {
                    current: Some(Release {
                        version: "0.1.0".into(),
                        target: None,
                    }),
                    bundled: release("0.2.0"),
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: true,
                },
            ),
            (
                "held gui, autostart-managed, systemd available: no second manager",
                Facts {
                    current: release("0.2.0"),
                    autostart_file: true,
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: false,
                },
            ),
            (
                "held gui without systemd",
                Facts {
                    current: release("0.2.0"),
                    systemd: false,
                    ..held(gui)
                },
                Plan::Keep {
                    channel: gui,
                    ensure_unit: false,
                },
            ),
            (
                "free, GUI layout and unit: start with systemd",
                Facts {
                    current: release("0.2.0"),
                    bundled: release("0.2.0"),
                    unit_file: true,
                    keg: true,
                    ..free()
                },
                Plan::Start {
                    update: None,
                    via: Via::Systemd,
                },
            ),
            (
                "free, GUI layout and unit, bundled newer: update first",
                Facts {
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    unit_file: true,
                    ..free()
                },
                Plan::Start {
                    update: Some("0.2.0".into()),
                    via: Via::Systemd,
                },
            ),
            (
                "free, GUI layout and autostart file: spawn",
                Facts {
                    current: release("0.2.0"),
                    autostart_file: true,
                    systemd: false,
                    ..free()
                },
                Plan::Start {
                    update: None,
                    via: Via::Spawn,
                },
            ),
            (
                "free, unit file but no systemd now: spawn",
                Facts {
                    current: release("0.2.0"),
                    unit_file: true,
                    systemd: false,
                    ..free()
                },
                Plan::Start {
                    update: None,
                    via: Via::Spawn,
                },
            ),
            (
                "free, Homebrew keg: brew services, never staged",
                Facts {
                    keg: true,
                    bundled: release("0.2.0"),
                    ..free()
                },
                Plan::BrewStart,
            ),
            (
                "free, Homebrew keg beats a GUI layout without a service file",
                Facts {
                    keg: true,
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    ..free()
                },
                Plan::BrewStart,
            ),
            (
                "free, bundled payload, systemd: install",
                Facts {
                    bundled: release("0.2.0"),
                    ..free()
                },
                Plan::Install {
                    activate: Some("0.2.0".into()),
                    via: Via::Systemd,
                },
            ),
            (
                "free, bundled payload, no systemd: autostart",
                Facts {
                    bundled: release("0.2.0"),
                    systemd: false,
                    ..free()
                },
                Plan::Install {
                    activate: Some("0.2.0".into()),
                    via: Via::Spawn,
                },
            ),
            (
                "free, newer current without a service file: register only",
                Facts {
                    current: release("0.3.0"),
                    bundled: release("0.2.0"),
                    ..free()
                },
                Plan::Install {
                    activate: None,
                    via: Via::Systemd,
                },
            ),
            (
                "free, older current without a service file: update and register",
                Facts {
                    current: release("0.1.0"),
                    bundled: release("0.2.0"),
                    ..free()
                },
                Plan::Install {
                    activate: Some("0.2.0".into()),
                    via: Via::Systemd,
                },
            ),
            ("free, nothing: not installed", free(), Plan::NotInstalled),
            (
                "unknown owner, nothing, unreachable: not installed",
                Facts {
                    owner: OwnerFact::Unknown,
                    ..free()
                },
                Plan::NotInstalled,
            ),
            (
                "unknown owner, reachable (a development jetd): keep",
                Facts {
                    owner: OwnerFact::Unknown,
                    reachable: true,
                    ..free()
                },
                Plan::Keep {
                    channel: None,
                    ensure_unit: false,
                },
            ),
        ];
        for (name, facts, plan) in cases {
            assert_eq!(decide(&facts), plan, "{name}");
        }
    }

    #[test]
    fn upgrade_compares_semver_not_text() {
        let facts = |current: &str, bundled: &str| Facts {
            current: release(current),
            bundled: release(bundled),
            ..free()
        };
        assert_eq!(
            upgrade(&facts("0.9.0", "0.10.0")).as_deref(),
            Some("0.10.0")
        );
        assert_eq!(
            upgrade(&facts("0.2.0-rc.1", "0.2.0")).as_deref(),
            Some("0.2.0")
        );
        assert_eq!(upgrade(&facts("0.2.0", "0.2.0-rc.1")), None);
        assert_eq!(upgrade(&facts("0.2.0", "0.2.0")), None);
        assert_eq!(upgrade(&facts("not-semver", "0.2.0")), None);
        assert_eq!(upgrade(&free()), None);
    }

    #[test]
    fn settled_views() {
        let gui_running = Facts {
            current: release("0.2.0"),
            previous: Some("0.1.0".into()),
            running: Some("0.2.0".into()),
            unit_file: true,
            bundled: release("0.2.0"),
            ..held(Some(Channel::Gui))
        };
        let view = settle(
            &gui_running,
            &Outcome {
                channel: Some(Channel::Gui),
                action: Some(Action::Installed),
                ..Outcome::default()
            },
        );
        assert_eq!(view.phase, Phase::Running);
        assert_eq!(view.channel, Some(Channel::Gui));
        assert_eq!(view.manager, Some(Manager::Systemd));
        assert_eq!(view.current_version.as_deref(), Some("0.2.0"));
        assert_eq!(view.previous_version.as_deref(), Some("0.1.0"));
        assert!(view.can_rollback);
        assert!(!view.can_repair);
        assert_eq!(view.last_action, Some(Action::Installed));

        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["phase"], "running");
        assert_eq!(json["channel"], "gui");
        assert_eq!(json["manager"], "systemd");
        assert_eq!(json["lastAction"], "installed");
        assert_eq!(json["bundledVersion"], "0.2.0");
        assert!(json["error"].is_null());

        let homebrew = settle(&held(Some(Channel::Homebrew)), &Outcome::default());
        assert_eq!(homebrew.manager, Some(Manager::BrewServices));
        assert!(!homebrew.can_rollback);

        let failed = settle(
            &Facts {
                bundled: release("0.2.0"),
                ..free()
            },
            &Outcome {
                channel: Some(Channel::Gui),
                action: Some(Action::Installed),
                error: Some(super::super::codes::install_failed()),
                not_installed: false,
            },
        );
        assert_eq!(failed.phase, Phase::Failed);
        assert_eq!(failed.last_action, None);
        assert!(failed.can_repair);
        assert_eq!(failed.error.unwrap().code, "service.install_failed");

        let missing = settle(
            &free(),
            &Outcome {
                not_installed: true,
                ..Outcome::default()
            },
        );
        assert_eq!(missing.phase, Phase::NotInstalled);
        assert!(!missing.can_repair);

        let stopped = settle(
            &Facts {
                current: release("0.2.0"),
                autostart_file: true,
                ..free()
            },
            &Outcome {
                channel: Some(Channel::Gui),
                ..Outcome::default()
            },
        );
        assert_eq!(stopped.phase, Phase::Stopped);
        assert_eq!(stopped.manager, Some(Manager::Autostart));
        assert!(stopped.can_repair);
    }
}
