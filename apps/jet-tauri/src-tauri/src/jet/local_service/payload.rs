//! The core payload this build carries: the unmodified release archive
//! `jet-core-<version>-<label>.tar.gz`, bundled as the resource
//! `jet-core.tar.gz` (absent from development builds).
//!
//! It stays an archive inside the package: linuxdeploy rewrites the rpath
//! of every ELF file under an AppImage's `usr/lib`, which would break the
//! manifest digests `jetd core stage` checks. It is unpacked with the system
//! `tar` into a fresh owner-only directory under the app cache, checked,
//! staged from there, and removed when the pass ends.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use uuid::Uuid;

use super::{
    codes,
    core_cli::safe_version,
    process::{Invocation, Processes},
};
use crate::jet::errors::PublicError;

/// The resource name `tauri.release.conf.json` maps the archive to.
pub(crate) const RESOURCE: &str = "jet-core.tar.gz";
/// The release archive is about 10 MiB per architecture (ADR-0054 caps the
/// four executables at 30 MiB); anything far larger is not ours.
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
/// `manifest.json`'s own limit in `jetd`.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Unpacking about 25 MiB.
const EXTRACT_LIMIT: Duration = Duration::from_secs(120);
/// Extraction directories live here, under the app cache directory.
const SCRATCH: &str = "local-service";
const PREFIX: &str = "payload-";

/// The archive this build carries and what it must contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundledPayload {
    pub(crate) archive: PathBuf,
    /// This app's version (ADR-0053: one release version for every product).
    pub(crate) version: String,
    /// The release label for this build's architecture.
    pub(crate) target: String,
}

impl BundledPayload {
    /// Whether the archive is present (it is absent in development builds).
    pub(crate) fn present(&self) -> bool {
        fs::metadata(&self.archive).is_ok_and(|metadata| metadata.is_file())
    }
}

/// The release label of the running build, as the payload manifest names it.
pub(crate) fn build_target() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// An unpacked, checked payload. Dropping it removes the directory.
#[derive(Debug)]
pub(crate) struct Extracted {
    directory: PathBuf,
    /// The archive's single top-level directory, passed to `stage`.
    pub(crate) payload: PathBuf,
    pub(crate) jetd: PathBuf,
}

impl Drop for Extracted {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    target: String,
}

/// Removes extraction directories a crashed pass left behind. Called under
/// the cross-process lock, so no other instance is using them.
pub(crate) fn clear_stale(scratch: &Path) {
    let Ok(entries) = fs::read_dir(scratch.join(SCRATCH)) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(PREFIX) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// A fresh owner-only directory for one extraction.
fn fresh_directory(scratch: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let parent = scratch.join(SCRATCH);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&parent)?;
    let directory = parent.join(format!("{PREFIX}{}", Uuid::new_v4().simple()));
    // Not recursive: the directory must be new.
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    Ok(directory)
}

pub(crate) fn extract_invocation(tar: &Path, archive: &Path, directory: &Path) -> Invocation {
    Invocation::new(
        tar,
        [
            "--no-same-owner".as_ref(),
            "--no-same-permissions".as_ref(),
            "-xzf".as_ref(),
            archive.as_os_str(),
            "-C".as_ref(),
            directory.as_os_str(),
        ],
    )
}

/// Unpacks and checks the bundled archive.
pub(crate) async fn extract(
    processes: &dyn Processes,
    tar: Option<&Path>,
    bundled: &BundledPayload,
    scratch: &Path,
) -> Result<Extracted, PublicError> {
    let tar = tar.ok_or_else(codes::payload_invalid)?;
    let archive = fs::metadata(&bundled.archive).map_err(|_| codes::payload_invalid())?;
    if !archive.is_file() || archive.len() > MAX_ARCHIVE_BYTES {
        return Err(codes::payload_invalid());
    }
    let directory = fresh_directory(scratch).map_err(|_| codes::install_failed())?;
    // From here on the directory is removed on every path.
    let guard = RemoveOnDrop(Some(directory.clone()));
    let finished = processes
        .run(
            &extract_invocation(tar, &bundled.archive, &directory),
            EXTRACT_LIMIT,
        )
        .await
        .map_err(|_| codes::payload_invalid())?;
    if !finished.succeeded() {
        return Err(codes::payload_invalid());
    }
    let payload = check(&directory, bundled).ok_or_else(codes::payload_invalid)?;
    Ok(Extracted {
        directory: guard.disarm(),
        jetd: payload.join("jetd"),
        payload,
    })
}

/// Removes a directory unless disarmed; `Extracted` takes over from it.
struct RemoveOnDrop(Option<PathBuf>);

impl RemoveOnDrop {
    fn disarm(mut self) -> PathBuf {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take() {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

/// The payload directory when `directory` holds exactly one real directory
/// whose manifest names this build's version and target and whose `jetd`
/// is a regular file. `stage` verifies every digest itself.
pub(crate) fn check(directory: &Path, expected: &BundledPayload) -> Option<PathBuf> {
    let mut entries = fs::read_dir(directory).ok()?;
    let only = entries.next()?.ok()?;
    if entries.next().is_some() {
        return None;
    }
    let payload = only.path();
    if !fs::symlink_metadata(&payload).ok()?.is_dir() {
        return None;
    }
    let manifest_path = payload.join("manifest.json");
    let metadata = fs::symlink_metadata(&manifest_path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(&manifest_path)
        .ok()?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    if safe_version(&manifest.version)? != expected.version || manifest.target != expected.target {
        return None;
    }
    if !fs::symlink_metadata(payload.join("jetd")).ok()?.is_file() {
        return None;
    }
    Some(payload)
}

/// The target `~/.jet/core/current/manifest.json` names, when readable.
pub(crate) fn current_target(home: &Path) -> Option<String> {
    let path = home.join("core/current/manifest.json");
    let mut bytes = Vec::new();
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    safe_version(&manifest.target)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const TARGET: &str = "x86_64-unknown-linux-gnu";

    /// Writes what the release archive unpacks to.
    pub(crate) fn write_payload(directory: &Path, version: &str, target: &str) {
        let payload = directory.join(format!("jet-core-{version}-{target}"));
        fs::create_dir_all(&payload).unwrap();
        for name in ["jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"] {
            fs::write(payload.join(name), b"\x7fELF").unwrap();
        }
        fs::write(
            payload.join("manifest.json"),
            format!(r#"{{"version":"{version}","target":"{target}","protocol_major":1,"protocol_minor":44,"schema_version":1,"executables":{{}}}}"#),
        )
        .unwrap();
        fs::write(payload.join("SHA256SUMS"), b"").unwrap();
    }

    fn expected() -> BundledPayload {
        BundledPayload {
            archive: PathBuf::from("/nonexistent/jet-core.tar.gz"),
            version: "0.2.0".into(),
            target: TARGET.into(),
        }
    }

    #[test]
    fn a_release_payload_passes() {
        let directory = tempfile::tempdir().unwrap();
        write_payload(directory.path(), "0.2.0", TARGET);
        let payload = check(directory.path(), &expected()).unwrap();
        assert!(payload.ends_with("jet-core-0.2.0-x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn anything_else_is_refused() {
        type Damage = fn(&Path);
        let cases: [(&str, Damage); 8] = [
            ("another version", |d| write_payload(d, "0.1.0", TARGET)),
            ("another target", |d| {
                write_payload(d, "0.2.0", "aarch64-unknown-linux-gnu")
            }),
            ("two top-level entries", |d| {
                write_payload(d, "0.2.0", TARGET);
                fs::write(d.join("extra"), b"").unwrap();
            }),
            ("empty", |_| {}),
            ("a loose file", |d| fs::write(d.join("jetd"), b"").unwrap()),
            ("no jetd", |d| {
                write_payload(d, "0.2.0", TARGET);
                fs::remove_file(d.join(format!("jet-core-0.2.0-{TARGET}/jetd"))).unwrap();
            }),
            ("oversized manifest", |d| {
                write_payload(d, "0.2.0", TARGET);
                fs::write(
                    d.join(format!("jet-core-0.2.0-{TARGET}/manifest.json")),
                    vec![b' '; 65 * 1024],
                )
                .unwrap();
            }),
            ("a symlinked top-level directory", |d| {
                let elsewhere = d
                    .parent()
                    .unwrap()
                    .join(format!("elsewhere-{}", Uuid::new_v4().simple()));
                write_payload(&elsewhere, "0.2.0", TARGET);
                std::os::unix::fs::symlink(
                    elsewhere.join(format!("jet-core-0.2.0-{TARGET}")),
                    d.join("jet-core"),
                )
                .unwrap();
            }),
        ];
        for (name, damage) in cases {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("x");
            fs::create_dir(&directory).unwrap();
            damage(&directory);
            assert_eq!(check(&directory, &expected()), None, "{name}");
        }
    }

    #[test]
    fn stale_extractions_are_cleared() {
        let scratch = tempfile::tempdir().unwrap();
        let first = fresh_directory(scratch.path()).unwrap();
        let second = fresh_directory(scratch.path()).unwrap();
        assert_ne!(first, second);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::write(scratch.path().join(SCRATCH).join("keep"), b"").unwrap();
        clear_stale(scratch.path());
        assert!(!first.exists() && !second.exists());
        assert!(scratch.path().join(SCRATCH).join("keep").exists());
    }

    #[test]
    fn the_extract_command_is_fixed() {
        assert_eq!(
            extract_invocation(
                Path::new("/usr/bin/tar"),
                Path::new("/usr/lib/Jet/jet-core.tar.gz"),
                Path::new("/c/local-service/payload-1")
            )
            .argv(),
            [
                "/usr/bin/tar",
                "--no-same-owner",
                "--no-same-permissions",
                "-xzf",
                "/usr/lib/Jet/jet-core.tar.gz",
                "-C",
                "/c/local-service/payload-1"
            ]
        );
    }

    #[test]
    fn current_target_reads_the_active_manifest() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(current_target(home.path()), None);
        let version = home.path().join("core/versions/0.2.0");
        fs::create_dir_all(&version).unwrap();
        fs::write(
            version.join("manifest.json"),
            format!(r#"{{"version":"0.2.0","target":"{TARGET}"}}"#),
        )
        .unwrap();
        std::os::unix::fs::symlink("versions/0.2.0", home.path().join("core/current")).unwrap();
        assert_eq!(current_target(home.path()).as_deref(), Some(TARGET));
    }

    #[test]
    fn the_build_has_a_release_label() {
        if cfg!(target_os = "linux") {
            assert!(build_target().is_some_and(|label| label.ends_with("-unknown-linux-gnu")));
        }
    }
}
