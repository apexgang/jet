//! `jetd core`, run from a `jetd` the shell chose (`docs/core-distribution.md`).
//!
//! Every subcommand prints one JSON object on standard output. Exit codes:
//! `0` success, `1` failure (standard error only, never read here), `2`
//! usage, `3` refused, `4` the owner did not relinquish the Plane in time.
//! The shell passes `--home` explicitly, names only versions it validated
//! itself, and turns every outcome into a stable `service.*` code.
use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};

use super::{
    codes,
    process::{Finished, Invocation, Processes},
};
use crate::jet::errors::PublicError;

/// `status` reads the layout, the live helpers (through `/bin/ps`) and the
/// lock.
pub(super) const STATUS_LIMIT: Duration = Duration::from_secs(20);
/// `stage` hashes and copies four executables of a few MiB each.
pub(super) const STAGE_LIMIT: Duration = Duration::from_secs(120);
/// `activate` and `rollback` wait up to their default 30 s drain, then
/// switch two links.
pub(super) const SWITCH_LIMIT: Duration = Duration::from_secs(120);

/// The installation channel a daemon's lock metadata names (ADR-0026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Channel {
    Gui,
    Homebrew,
    Development,
}

impl Channel {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "gui" => Some(Self::Gui),
            "homebrew" => Some(Self::Homebrew),
            "development" => Some(Self::Development),
            _ => None,
        }
    }
}

/// Who holds the Plane's lifetime lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Owner {
    Free,
    /// `channel` and `version` are `None` when the lock metadata is missing
    /// or names something this build does not know.
    Held {
        channel: Option<Channel>,
        version: Option<String>,
    },
}

/// What `jetd core status` reports that the shell uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoreStatus {
    pub(crate) current: Option<String>,
    pub(crate) previous: Option<String>,
    pub(crate) owner: Owner,
}

#[derive(Deserialize)]
struct RawStatus {
    status: String,
    current: Option<String>,
    previous: Option<String>,
    owner: RawOwner,
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RawOwner {
    Free,
    Held { daemon: Option<RawDaemon> },
}

#[derive(Deserialize)]
struct RawDaemon {
    version: String,
    channel: String,
}

#[derive(Deserialize)]
struct RawOutcome {
    status: String,
    version: Option<String>,
    code: Option<String>,
}

/// A release version as `jetd` names it: `[A-Za-z0-9.+_-]`, no leading dot,
/// at most 64 bytes (the manifest's own rule). Anything else is neither
/// passed to a command nor shown.
pub(crate) fn safe_version(value: &str) -> Option<String> {
    let plain = !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+_-".contains(&byte));
    plain.then(|| value.to_owned())
}

fn home_args(home: &Path) -> [&std::ffi::OsStr; 2] {
    ["--home".as_ref(), home.as_os_str()]
}

pub(super) fn status_invocation(jetd: &Path, home: &Path) -> Invocation {
    let mut args: Vec<&std::ffi::OsStr> = vec!["core".as_ref(), "status".as_ref()];
    args.extend(home_args(home));
    Invocation::new(jetd, args).capturing()
}

pub(super) fn stage_invocation(jetd: &Path, payload: &Path, home: &Path) -> Invocation {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "core".as_ref(),
        "stage".as_ref(),
        "--payload".as_ref(),
        payload.as_os_str(),
    ];
    args.extend(home_args(home));
    Invocation::new(jetd, args).capturing()
}

pub(super) fn activate_invocation(jetd: &Path, version: &str, home: &Path) -> Invocation {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "core".as_ref(),
        "activate".as_ref(),
        "--version".as_ref(),
        version.as_ref(),
    ];
    args.extend(home_args(home));
    Invocation::new(jetd, args).capturing()
}

pub(super) fn rollback_invocation(jetd: &Path, home: &Path) -> Invocation {
    let mut args: Vec<&std::ffi::OsStr> = vec!["core".as_ref(), "rollback".as_ref()];
    args.extend(home_args(home));
    Invocation::new(jetd, args).capturing()
}

/// Parses a successful `status` report. Versions that are not plain names
/// are dropped rather than trusted.
pub(super) fn parse_status(finished: &Finished) -> Result<CoreStatus, PublicError> {
    if !finished.succeeded() {
        return Err(codes::status_failed());
    }
    let raw: RawStatus =
        serde_json::from_slice(&finished.stdout).map_err(|_| codes::status_failed())?;
    if raw.status != "ok" {
        return Err(codes::status_failed());
    }
    let owner = match raw.owner {
        RawOwner::Free => Owner::Free,
        RawOwner::Held { daemon: None } => Owner::Held {
            channel: None,
            version: None,
        },
        RawOwner::Held {
            daemon: Some(daemon),
        } => Owner::Held {
            channel: Channel::parse(&daemon.channel),
            version: safe_version(&daemon.version),
        },
    };
    Ok(CoreStatus {
        current: raw.current.as_deref().and_then(safe_version),
        previous: raw.previous.as_deref().and_then(safe_version),
        owner,
    })
}

/// A successful `stage`: the version it staged.
pub(super) fn parse_staged(finished: &Finished) -> Result<String, PublicError> {
    match outcome(finished) {
        Some(raw) if finished.succeeded() && raw.status == "staged" => raw
            .version
            .as_deref()
            .and_then(safe_version)
            .ok_or_else(codes::install_failed),
        // A payload `stage` rejects (digest, manifest, a different payload
        // under a staged name) exits 1.
        _ => Err(codes::install_failed()),
    }
}

/// `activate` and `rollback`: the version `current` names now, or the
/// refusal as a stable code.
pub(super) fn parse_switched(
    finished: &Finished,
    failed: fn() -> PublicError,
) -> Result<String, PublicError> {
    let raw = outcome(finished);
    match finished.code {
        Some(0) => raw
            .filter(|raw| raw.status == "activated")
            .and_then(|raw| raw.version.as_deref().and_then(safe_version))
            .ok_or_else(failed),
        Some(3) => Err(match raw.and_then(|raw| raw.code).as_deref() {
            Some("channel_owned") => codes::channel_owned(),
            Some("no_previous_version") => codes::rollback_unavailable(),
            Some("owner_unknown") => codes::owner_unknown(),
            // target_mismatch, protocol_major_pinned, schema_outside_pair.
            _ => codes::switch_refused(),
        }),
        Some(4) => Err(codes::drain_timeout()),
        _ => Err(failed()),
    }
}

fn outcome(finished: &Finished) -> Option<RawOutcome> {
    serde_json::from_slice(&finished.stdout).ok()
}

pub(super) async fn status(
    processes: &dyn Processes,
    jetd: &Path,
    home: &Path,
) -> Result<CoreStatus, PublicError> {
    let finished = processes
        .run(&status_invocation(jetd, home), STATUS_LIMIT)
        .await
        .map_err(|_| codes::status_failed())?;
    parse_status(&finished)
}

pub(super) async fn stage(
    processes: &dyn Processes,
    jetd: &Path,
    payload: &Path,
    home: &Path,
) -> Result<String, PublicError> {
    let finished = processes
        .run(&stage_invocation(jetd, payload, home), STAGE_LIMIT)
        .await
        .map_err(|_| codes::install_failed())?;
    parse_staged(&finished)
}

pub(super) async fn activate(
    processes: &dyn Processes,
    jetd: &Path,
    version: &str,
    home: &Path,
) -> Result<String, PublicError> {
    let finished = processes
        .run(&activate_invocation(jetd, version, home), SWITCH_LIMIT)
        .await
        .map_err(|_| codes::install_failed())?;
    parse_switched(&finished, codes::install_failed)
}

pub(super) async fn rollback(
    processes: &dyn Processes,
    jetd: &Path,
    home: &Path,
) -> Result<String, PublicError> {
    let finished = processes
        .run(&rollback_invocation(jetd, home), SWITCH_LIMIT)
        .await
        .map_err(|_| codes::rollback_failed())?;
    parse_switched(&finished, codes::rollback_failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(code: i32, stdout: &str) -> Finished {
        Finished {
            code: Some(code),
            stdout: stdout.as_bytes().to_vec(),
        }
    }

    #[test]
    fn status_reports_owner_versions_and_channel() {
        let held = finished(
            0,
            r#"{"status":"ok","current":"0.2.0","previous":"0.1.0","staged":["0.1.0","0.2.0"],"live_helpers":[],"owner":{"state":"held","daemon":{"pid":7,"version":"0.2.0","channel":"homebrew"}}}"#,
        );
        assert_eq!(
            parse_status(&held).unwrap(),
            CoreStatus {
                current: Some("0.2.0".into()),
                previous: Some("0.1.0".into()),
                owner: Owner::Held {
                    channel: Some(Channel::Homebrew),
                    version: Some("0.2.0".into()),
                },
            }
        );
        let free = finished(
            0,
            r#"{"status":"ok","current":null,"previous":null,"staged":[],"live_helpers":[],"owner":{"state":"free"}}"#,
        );
        assert_eq!(parse_status(&free).unwrap().owner, Owner::Free);
    }

    /// Unknown metadata, a future channel and unsafe version names never
    /// become trusted facts.
    #[test]
    fn status_drops_what_it_cannot_trust() {
        let unknown = finished(
            0,
            r#"{"status":"ok","current":"../../etc","previous":null,"staged":[],"live_helpers":[],"owner":{"state":"held","daemon":{"pid":7,"version":"0.2.0 --x","channel":"system"}}}"#,
        );
        assert_eq!(
            parse_status(&unknown).unwrap(),
            CoreStatus {
                current: None,
                previous: None,
                owner: Owner::Held {
                    channel: None,
                    version: None,
                },
            }
        );
        let no_metadata = finished(
            0,
            r#"{"status":"ok","current":null,"previous":null,"staged":[],"live_helpers":[],"owner":{"state":"held","daemon":null}}"#,
        );
        assert_eq!(
            parse_status(&no_metadata).unwrap().owner,
            Owner::Held {
                channel: None,
                version: None
            }
        );
        for bad in [finished(1, ""), finished(0, "{"), finished(0, "[]")] {
            assert_eq!(
                parse_status(&bad).unwrap_err().code,
                "service.status_failed"
            );
        }
    }

    #[test]
    fn switch_outcomes_map_to_stable_codes() {
        let cases: [(i32, &str, Result<&str, &str>); 9] = [
            (
                0,
                r#"{"status":"activated","version":"0.2.0","previous":"0.1.0","drained":null,"live_helpers":0}"#,
                Ok("0.2.0"),
            ),
            (0, r#"{"status":"refused"}"#, Err("service.install_failed")),
            (
                3,
                r#"{"status":"refused","code":"channel_owned","owner":{"pid":1,"version":"0.2.0","channel":"homebrew"}}"#,
                Err("service.channel_owned"),
            ),
            (
                3,
                r#"{"status":"refused","code":"no_previous_version"}"#,
                Err("service.rollback_unavailable"),
            ),
            (
                3,
                r#"{"status":"refused","code":"owner_unknown"}"#,
                Err("service.owner_unknown"),
            ),
            (
                3,
                r#"{"status":"refused","code":"schema_outside_pair","candidate":1,"store":2,"previous":null}"#,
                Err("service.update_refused"),
            ),
            (
                4,
                r#"{"status":"drain_timeout","owner":{},"timeout_secs":30}"#,
                Err("service.drain_timeout"),
            ),
            (1, "", Err("service.install_failed")),
            (2, "", Err("service.install_failed")),
        ];
        for (code, stdout, expected) in cases {
            let result = parse_switched(&finished(code, stdout), codes::install_failed);
            match expected {
                Ok(version) => assert_eq!(result.unwrap(), version, "{code} {stdout}"),
                Err(expected) => assert_eq!(result.unwrap_err().code, expected, "{code} {stdout}"),
            }
        }
        let signalled = Finished {
            code: None,
            stdout: Vec::new(),
        };
        assert_eq!(
            parse_switched(&signalled, codes::rollback_failed)
                .unwrap_err()
                .code,
            "service.rollback_failed"
        );
    }

    #[test]
    fn staged_reports_its_version() {
        assert_eq!(
            parse_staged(&finished(
                0,
                r#"{"status":"staged","version":"0.2.0","target":"x86_64-unknown-linux-gnu"}"#
            ))
            .unwrap(),
            "0.2.0"
        );
        assert_eq!(
            parse_staged(&finished(1, "")).unwrap_err().code,
            "service.install_failed"
        );
    }

    #[test]
    fn invocations_are_fixed_argument_lists() {
        let jetd = Path::new("/home/u/.jet/core/current/jetd");
        let home = Path::new("/home/u/.jet");
        assert_eq!(
            status_invocation(jetd, home).argv(),
            [
                "/home/u/.jet/core/current/jetd",
                "core",
                "status",
                "--home",
                "/home/u/.jet"
            ]
        );
        assert_eq!(
            activate_invocation(jetd, "0.2.0", home).argv()[1..],
            [
                "core",
                "activate",
                "--version",
                "0.2.0",
                "--home",
                "/home/u/.jet"
            ]
        );
        assert_eq!(
            stage_invocation(jetd, Path::new("/cache/p/jet-core"), home).argv()[1..],
            [
                "core",
                "stage",
                "--payload",
                "/cache/p/jet-core",
                "--home",
                "/home/u/.jet"
            ]
        );
        assert_eq!(
            rollback_invocation(jetd, home).argv()[1..],
            ["core", "rollback", "--home", "/home/u/.jet"]
        );
        assert!(status_invocation(jetd, home).capture);
    }

    #[test]
    fn versions_are_plain_names() {
        for good in ["0.2.0", "0.2.0-rc.1+build_7", "1"] {
            assert_eq!(safe_version(good).as_deref(), Some(good));
        }
        for bad in ["", ".hidden", "../x", "0.2.0 ", "a/b", &"9".repeat(65)] {
            assert_eq!(safe_version(bad), None, "{bad}");
        }
    }
}
