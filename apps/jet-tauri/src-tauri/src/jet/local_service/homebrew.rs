//! The Homebrew-managed core: the `apexgang/tap/jetd` keg and the `brew`
//! that can start its service. ADR-0026: the app may ask Homebrew to start
//! its own service, but never stages, activates or updates that core.
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// A keg the app may use: both paths canonical, regular, executable, and
/// owned by this user or root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Keg {
    pub(crate) jetd: PathBuf,
    pub(crate) brew: PathBuf,
}

/// Where Homebrew on Linux lives, in lookup order: `$HOMEBREW_PREFIX` when
/// it is absolute, the default shared prefix, then the per-user one.
pub(crate) fn prefixes(homebrew_prefix: Option<OsString>, user_home: &Path) -> Vec<PathBuf> {
    let mut prefixes = Vec::new();
    if let Some(prefix) = homebrew_prefix.map(PathBuf::from) {
        if prefix.is_absolute() {
            prefixes.push(prefix);
        }
    }
    for fallback in [
        PathBuf::from("/home/linuxbrew/.linuxbrew"),
        user_home.join(".linuxbrew"),
    ] {
        if !prefixes.contains(&fallback) {
            prefixes.push(fallback);
        }
    }
    prefixes
}

/// The first prefix whose `opt/jetd/bin/jetd` and `bin/brew` are trusted.
pub(crate) fn find(prefixes: &[PathBuf], owner_uid: u32) -> Option<Keg> {
    prefixes.iter().find_map(|prefix| {
        Some(Keg {
            jetd: trusted_executable(&prefix.join("opt/jetd/bin/jetd"), owner_uid)?,
            brew: trusted_executable(&prefix.join("bin/brew"), owner_uid)?,
        })
    })
}

/// ASVS 12.3.1: canonicalize first, then require a regular executable file
/// owned by this user or root, so another local user cannot plant one.
pub(crate) fn trusted_executable(path: &Path, owner_uid: u32) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let metadata = fs::metadata(&canonical).ok()?;
    let trusted = metadata.is_file()
        && metadata.permissions().mode() & 0o111 != 0
        && (metadata.uid() == owner_uid || metadata.uid() == 0);
    trusted.then_some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn uid(path: &Path) -> u32 {
        fs::metadata(path).unwrap().uid()
    }

    #[test]
    fn prefixes_follow_the_environment_then_the_defaults() {
        let home = Path::new("/home/u");
        assert_eq!(
            prefixes(Some("/opt/brew".into()), home),
            [
                PathBuf::from("/opt/brew"),
                PathBuf::from("/home/linuxbrew/.linuxbrew"),
                PathBuf::from("/home/u/.linuxbrew")
            ]
        );
        assert_eq!(
            prefixes(Some("relative/brew".into()), home),
            [
                PathBuf::from("/home/linuxbrew/.linuxbrew"),
                PathBuf::from("/home/u/.linuxbrew")
            ]
        );
        assert_eq!(
            prefixes(Some("/home/linuxbrew/.linuxbrew".into()), home).len(),
            2
        );
    }

    #[test]
    fn a_keg_needs_both_trusted_executables() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("brew");
        let owner = uid(root.path());
        let prefixes = [root.path().join("missing"), prefix.clone()];
        assert_eq!(find(&prefixes, owner), None);

        // opt/jetd is a link into the Cellar, as Homebrew makes it.
        let cellar = prefix.join("Cellar/jetd/0.2.0");
        executable(&cellar.join("bin/jetd"));
        fs::create_dir_all(prefix.join("opt")).unwrap();
        std::os::unix::fs::symlink(&cellar, prefix.join("opt/jetd")).unwrap();
        assert_eq!(find(&prefixes, owner), None, "no brew yet");

        executable(&prefix.join("bin/brew"));
        let keg = find(&prefixes, owner).unwrap();
        assert_eq!(keg.jetd, fs::canonicalize(cellar.join("bin/jetd")).unwrap());
        assert_eq!(keg.brew, fs::canonicalize(prefix.join("bin/brew")).unwrap());

        // Owned by someone else (neither this user nor root).
        assert_eq!(find(&prefixes, owner.wrapping_add(1)).is_some(), owner == 0);
    }

    #[test]
    fn untrusted_files_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let owner = uid(root.path());
        let plain = root.path().join("plain");
        fs::write(&plain, b"").unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(trusted_executable(&plain, owner), None, "not executable");
        assert_eq!(trusted_executable(root.path(), owner), None, "a directory");
        assert_eq!(
            trusted_executable(&root.path().join("missing"), owner),
            None
        );
    }
}
