//! The search PATH a Craft and its helper find their Harness on (ADR-0107).
//!
//! A Craft names its Harness by a bare name such as `claude` or `codex`, and
//! the helper resolves it through the PATH `jetd` hands down. A service
//! manager starts `jetd` with a minimal PATH that misses where the Harness
//! installers put their command-line tools, so `jetd` appends those well-known
//! directories to whatever PATH it inherited. No shell is invoked and no
//! version manager's version is guessed: only the shims directories that
//! asdf and mise keep in one place are appended (ADR-0026, ADR-0056).

use std::{
	ffi::{OsStr, OsString},
	os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
	path::{Path, PathBuf},
};

/// Per-user directories Harness installers use, under the user's home.
const USER_DIRECTORIES: [&str; 7] = [
	".local/bin",
	".claude/local",
	".npm-global/bin",
	".bun/bin",
	".volta/bin",
	".asdf/shims",
	".local/share/mise/shims",
];

/// System directories Harness installers and package managers use.
const SYSTEM_DIRECTORIES: [&str; 5] = [
	"/opt/homebrew/bin",
	"/home/linuxbrew/.linuxbrew/bin",
	"/usr/local/bin",
	"/usr/bin",
	"/bin",
];

/// Gives a process that may launch a Harness the Harness search PATH.
///
/// The PATH is built for each process, so a Harness directory created
/// after `jetd` started is found without a restart.
pub(crate) fn apply(command: &mut tokio::process::Command) {
	let euid = rustix::process::geteuid().as_raw();
	command.env(
		"PATH",
		harness_path(
			std::env::var_os("PATH").as_deref(),
			std::env::var_os("HOME").map(PathBuf::from).as_deref(),
			|directory| trusted_directory(directory, euid),
		),
	);
}

/// The inherited entries first, so a service definition or an owner's
/// override still wins, then the well-known Harness directories that
/// `appendable` accepts. Only absolute entries are kept: an empty or
/// relative entry would resolve against the working directory a Harness
/// runs in.
fn harness_path(
	inherited: Option<&OsStr>,
	home: Option<&Path>,
	appendable: impl Fn(&Path) -> bool,
) -> OsString {
	let home = home.filter(|home| home.is_absolute());
	let appended = home
		.into_iter()
		.flat_map(|home| {
			USER_DIRECTORIES
				.iter()
				.map(move |directory| home.join(directory))
		})
		.chain(SYSTEM_DIRECTORIES.iter().map(PathBuf::from))
		.filter(|directory| appendable(directory));
	let mut entries: Vec<PathBuf> = Vec::new();
	for entry in inherited
		.map(std::env::split_paths)
		.into_iter()
		.flatten()
		.chain(appended)
	{
		if entry.is_absolute()
			&& !entry.as_os_str().as_bytes().contains(&b':')
			&& !entries.contains(&entry)
		{
			entries.push(entry);
		}
	}
	std::env::join_paths(entries).unwrap_or_default()
}

/// Where a path component sits relative to the directory being appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Component {
	/// The directory searched for the Harness itself.
	Directory,
	/// A directory above it.
	Ancestor,
}

/// Whether a directory `jetd` did not inherit may be searched for a Harness
/// run with this user's credentials: it exists, and no other user can put a
/// command in it or swap it out. On a shared host another account may own
/// a Homebrew prefix such as `/home/linuxbrew/.linuxbrew`, and its `claude`
/// must not stand in for this user's own.
fn trusted_directory(directory: &Path, euid: u32) -> bool {
	let Ok(canonical) = std::fs::canonicalize(directory) else {
		return false;
	};
	canonical.ancestors().enumerate().all(|(depth, path)| {
		std::fs::symlink_metadata(path).is_ok_and(|metadata| {
			metadata.is_dir()
				&& trusted(
					metadata.uid(),
					metadata.mode(),
					euid,
					if depth == 0 {
						Component::Directory
					} else {
						Component::Ancestor
					},
				)
		})
	})
}

/// Root or this user owns the component, and others cannot write to it.
/// A sticky ancestor such as `/tmp` is accepted: others may write there but
/// cannot rename or remove what they do not own. Group write is accepted,
/// because Homebrew makes its prefix writable by the admin group and the
/// user's own login shell searches it the same way.
fn trusted(uid: u32, mode: u32, euid: u32, component: Component) -> bool {
	const OTHERS_WRITE: u32 = 0o002;
	const STICKY: u32 = 0o1000;
	(uid == 0 || uid == euid)
		&& (mode & OTHERS_WRITE == 0
			|| (component == Component::Ancestor && mode & STICKY != 0))
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use std::os::unix::fs::PermissionsExt as _;

	const SYSTEM: &str = "/opt/homebrew/bin:/home/linuxbrew/.linuxbrew/bin:/usr/local/bin:/usr/bin:/bin";

	fn path(inherited: Option<&str>, home: Option<&str>) -> Option<String> {
		harness_path(inherited.map(OsStr::new), home.map(Path::new), |_| true)
			.into_string()
			.ok()
	}

	#[test]
	fn the_inherited_path_wins_and_harness_directories_follow_once() {
		assert_eq!(
			path(
				Some("/custom/bin:/usr/bin:/home/user/.local/bin:/custom/bin"),
				Some("/home/user"),
			),
			Some(
				"/custom/bin:/usr/bin:/home/user/.local/bin:\
				 /home/user/.claude/local:/home/user/.npm-global/bin:\
				 /home/user/.bun/bin:/home/user/.volta/bin:\
				 /home/user/.asdf/shims:/home/user/.local/share/mise/shims:\
				 /opt/homebrew/bin:/home/linuxbrew/.linuxbrew/bin:\
				 /usr/local/bin:/bin"
					.to_owned()
			)
		);
	}

	#[test]
	fn without_an_absolute_home_only_system_directories_are_appended() {
		assert_eq!(
			path(Some("/custom/bin"), None),
			Some(format!("/custom/bin:{SYSTEM}"))
		);
		assert_eq!(
			path(Some("/custom/bin"), Some("relative/home")),
			Some(format!("/custom/bin:{SYSTEM}"))
		);
	}

	#[test]
	fn empty_and_relative_inherited_entries_are_dropped() {
		assert_eq!(path(Some(""), None), Some(SYSTEM.to_owned()));
		assert_eq!(path(None, None), Some(SYSTEM.to_owned()));
		assert_eq!(
			path(Some("::bin:/custom/bin:."), None),
			Some(format!("/custom/bin:{SYSTEM}"))
		);
	}

	#[test]
	fn only_appendable_directories_are_appended_and_inherited_ones_stay() {
		let appended = harness_path(
			Some(OsStr::new("/opt/homebrew/bin")),
			Some(Path::new("/home/user")),
			|directory| directory == Path::new("/home/user/.local/bin"),
		);

		assert_eq!(
			appended,
			OsString::from("/opt/homebrew/bin:/home/user/.local/bin")
		);
	}

	/// The shared-host case: another account's Homebrew prefix, or a
	/// directory anyone can write, never supplies this user's Harness.
	#[test]
	fn a_directory_another_user_owns_or_others_can_write_is_not_trusted() {
		let euid = 501;
		let checks = [
			(0, 0o755, Component::Directory),
			(euid, 0o775, Component::Directory),
			(502, 0o755, Component::Directory),
			(502, 0o755, Component::Ancestor),
			(euid, 0o777, Component::Directory),
			(0, 0o1777, Component::Directory),
			(0, 0o1777, Component::Ancestor),
			(0, 0o777, Component::Ancestor),
		]
		.map(|(uid, mode, component)| trusted(uid, mode, euid, component));

		assert_eq!(
			checks,
			[true, true, false, false, false, false, true, false]
		);
	}

	#[test]
	fn a_directory_is_trusted_only_when_it_exists_and_others_cannot_write_it() {
		let home = tempfile::tempdir().unwrap();
		let euid = rustix::process::geteuid().as_raw();
		let bin = home.path().join(".local/bin");
		let shared = home.path().join("shared");
		let inner = shared.join("inner");
		let linked = home.path().join("linked");
		std::fs::create_dir_all(&bin).unwrap();
		std::fs::create_dir_all(&inner).unwrap();
		std::fs::set_permissions(
			&shared,
			std::fs::Permissions::from_mode(0o777),
		)
		.unwrap();
		std::os::unix::fs::symlink(&shared, &linked).unwrap();

		assert_eq!(
			[bin, home.path().join(".bun/bin"), shared, linked, inner,]
				.map(|directory| trusted_directory(&directory, euid)),
			[true, false, false, false, false]
		);
	}
}
