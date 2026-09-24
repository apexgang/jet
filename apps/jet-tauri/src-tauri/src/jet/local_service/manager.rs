//! Service managers for a GUI-managed `jetd`: the systemd user unit, the
//! XDG autostart fallback with a detached daemon for this session, and
//! `brew services` for the Homebrew keg.
//!
//! The unit and autostart files are the packaging templates themselves
//! (`include_str!`), so there is one source of truth. Every command is a
//! fixed argument list from an absolute `systemctl` or `brew`.
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use uuid::Uuid;

use super::process::{Invocation, Processes};

/// `.github/packaging/linux/jetd.service`.
pub(crate) const UNIT_TEMPLATE: &str =
    include_str!("../../../../../../.github/packaging/linux/jetd.service");
/// `.github/packaging/linux/jetd-autostart.desktop`.
pub(crate) const AUTOSTART_TEMPLATE: &str =
    include_str!("../../../../../../.github/packaging/linux/jetd-autostart.desktop");
pub(crate) const UNIT_NAME: &str = "jetd.service";
/// The formula's full name: never the bare `jetd` (ADR-0026 channel).
pub(crate) const HOMEBREW_FORMULA: &str = "apexgang/tap/jetd";

/// `systemctl --user` answers within seconds; `start` waits for the job.
pub(crate) const SYSTEMCTL_LIMIT: Duration = Duration::from_secs(30);
/// `brew services start` may write and load its own unit first.
pub(crate) const BREW_LIMIT: Duration = Duration::from_secs(120);
/// Unit files and desktop entries are at most a few KiB.
const MAX_MANAGED_FILE: u64 = 64 * 1024;

pub(crate) fn unit_path(config_home: &Path) -> PathBuf {
    config_home.join("systemd/user").join(UNIT_NAME)
}

pub(crate) fn autostart_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart/jetd.desktop")
}

/// The first of the usual absolute locations that is a file.
pub(crate) fn locate(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
}

pub(crate) fn systemctl(systemctl: &Path, args: &[&str]) -> Invocation {
    Invocation::new(
        systemctl,
        std::iter::once("--user").chain(args.iter().copied()),
    )
}

/// `systemctl --user show-environment` succeeds only with a running user
/// manager and its bus.
pub(crate) async fn systemd_available(processes: &dyn Processes, systemctl: Option<&Path>) -> bool {
    let Some(systemctl) = systemctl else {
        return false;
    };
    processes
        .run(
            &self::systemctl(systemctl, &["show-environment"]),
            SYSTEMCTL_LIMIT,
        )
        .await
        .is_ok_and(|finished| finished.succeeded())
}

/// Runs `systemctl --user <args>`; `false` on any failure.
pub(crate) async fn run_systemctl(
    processes: &dyn Processes,
    systemctl: &Path,
    args: &[&str],
) -> bool {
    processes
        .run(&self::systemctl(systemctl, args), SYSTEMCTL_LIMIT)
        .await
        .is_ok_and(|finished| finished.succeeded())
}

pub(crate) fn brew_services_start(brew: &Path) -> Invocation {
    Invocation::new(brew, ["services", "start", HOMEBREW_FORMULA])
}

/// The `PATH` the unit gives `jetd` (Harness CLIs such as `claude` live in
/// `~/.local/bin`). A daemon this app starts itself gets the same one.
pub(crate) fn service_path(user_home: &Path) -> OsString {
    let mut path = user_home.join(".local/bin").into_os_string();
    path.push(":/usr/local/bin:/usr/bin:/bin");
    path
}

/// `current/jetd serve --channel gui`, as the unit and the autostart file
/// start it.
pub(crate) fn serve(jet_home: &Path, user_home: &Path) -> Invocation {
    Invocation::new(
        jet_home.join("core/current/jetd"),
        ["serve", "--channel", "gui"],
    )
    .with_path(service_path(user_home))
}

/// Replaces `path` with `bytes` (mode `mode`) unless it already holds
/// exactly them: a synced temporary file renamed over it. Returns whether
/// it wrote. Blocking.
pub(crate) fn write_if_changed(path: &Path, bytes: &[u8], mode: u32) -> io::Result<bool> {
    if read_small(path)?.as_deref() == Some(bytes) {
        return Ok(false);
    }
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    fs::create_dir_all(directory)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .to_string_lossy()
        .into_owned();
    let temporary = directory.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)))
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(true)
}

/// A small regular file's bytes; `None` when missing. Anything else
/// (a directory, an oversized file) is an error, never overwritten blindly.
fn read_small(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    if metadata.len() > MAX_MANAGED_FILE {
        return Ok(Some(Vec::new()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANAGED_FILE).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

pub(crate) fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unit refuses to start without an activated core, gives Harness
    /// CLIs in `~/.local/bin` a PATH, and keeps helpers alive (Wave 4 §A).
    #[test]
    fn the_unit_template_is_the_wave_4_unit() {
        for line in [
            "ConditionFileIsExecutable=%h/.jet/core/current/jetd",
            "Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin",
            "ExecStart=%h/.jet/core/current/jetd serve --channel gui",
            "KillMode=process",
            "Restart=always",
        ] {
            assert!(
                UNIT_TEMPLATE.lines().any(|candidate| candidate == line),
                "{line}"
            );
        }
        let unit_section = UNIT_TEMPLATE.split("[Service]").next().unwrap();
        assert!(unit_section.contains("ConditionFileIsExecutable="));
        assert!(AUTOSTART_TEMPLATE.contains("serve --channel gui"));
        assert_eq!(
            service_path(Path::new("/home/u")),
            "/home/u/.local/bin:/usr/local/bin:/usr/bin:/bin"
        );
    }

    #[test]
    fn managed_files_are_rewritten_only_when_they_differ() {
        let root = tempfile::tempdir().unwrap();
        let unit = unit_path(root.path());
        assert!(write_if_changed(&unit, b"one", 0o644).unwrap());
        assert_eq!(
            fs::metadata(&unit).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!write_if_changed(&unit, b"one", 0o644).unwrap());
        assert!(write_if_changed(&unit, b"two", 0o644).unwrap());
        assert_eq!(fs::read(&unit).unwrap(), b"two");
        let leftovers = fs::read_dir(unit.parent().unwrap())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);

        // A directory in the file's place is reported, not replaced.
        let blocked = autostart_path(root.path());
        fs::create_dir_all(&blocked).unwrap();
        assert!(write_if_changed(&blocked, b"x", 0o644).is_err());
        assert!(blocked.is_dir());
    }

    #[test]
    fn commands_are_fixed() {
        assert_eq!(
            systemctl(
                Path::new("/usr/bin/systemctl"),
                &["enable", "--now", UNIT_NAME]
            )
            .argv(),
            [
                "/usr/bin/systemctl",
                "--user",
                "enable",
                "--now",
                "jetd.service"
            ]
        );
        assert_eq!(
            brew_services_start(Path::new("/home/linuxbrew/.linuxbrew/bin/brew")).argv(),
            [
                "/home/linuxbrew/.linuxbrew/bin/brew",
                "services",
                "start",
                "apexgang/tap/jetd"
            ]
        );
        let serve = serve(Path::new("/home/u/.jet"), Path::new("/home/u"));
        assert_eq!(
            serve.argv(),
            [
                "/home/u/.jet/core/current/jetd",
                "serve",
                "--channel",
                "gui"
            ]
        );
        assert!(serve.path_env.is_some());
    }
}
