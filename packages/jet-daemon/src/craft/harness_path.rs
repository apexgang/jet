//! The search PATH a Craft and its helper find their Harness on (ADR-0107).
//!
//! A Craft names its Harness by a bare name such as `claude` or `codex`, and
//! the helper resolves it through the PATH `jetd` hands down. A service
//! manager starts `jetd` with a minimal PATH that misses where the Harness
//! installers put their command-line tools, so `jetd` appends those well-known
//! directories to whatever PATH it inherited. No shell is invoked and no
//! version manager is guessed (ADR-0026, ADR-0056).

use std::{
	ffi::{OsStr, OsString},
	os::unix::ffi::OsStrExt as _,
	path::{Path, PathBuf},
	sync::LazyLock,
};

/// Per-user directories Harness installers use, under the user's home.
const USER_DIRECTORIES: [&str; 5] = [
	".local/bin",
	".claude/local",
	".npm-global/bin",
	".bun/bin",
	".volta/bin",
];

/// System directories Harness installers and package managers use.
const SYSTEM_DIRECTORIES: [&str; 5] = [
	"/opt/homebrew/bin",
	"/home/linuxbrew/.linuxbrew/bin",
	"/usr/local/bin",
	"/usr/bin",
	"/bin",
];

static HARNESS_PATH: LazyLock<OsString> = LazyLock::new(|| {
	harness_path(
		std::env::var_os("PATH").as_deref(),
		std::env::var_os("HOME").map(PathBuf::from).as_deref(),
	)
});

/// Gives a process that may launch a Harness the Harness search PATH.
pub(crate) fn apply(command: &mut tokio::process::Command) {
	command.env("PATH", &*HARNESS_PATH);
}

/// The inherited entries first, so a service definition or an owner's
/// override still wins, then the well-known Harness directories, whether or
/// not they exist yet, so a Harness installed later is found without a
/// restart. Only absolute entries are kept: an empty or relative entry would
/// resolve against the working directory a Harness runs in.
fn harness_path(inherited: Option<&OsStr>, home: Option<&Path>) -> OsString {
	let home = home.filter(|home| home.is_absolute());
	let appended = home
		.into_iter()
		.flat_map(|home| {
			USER_DIRECTORIES
				.iter()
				.map(move |directory| home.join(directory))
		})
		.chain(SYSTEM_DIRECTORIES.iter().map(PathBuf::from));
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

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	const SYSTEM: &str = "/opt/homebrew/bin:/home/linuxbrew/.linuxbrew/bin:/usr/local/bin:/usr/bin:/bin";

	fn path(inherited: Option<&str>, home: Option<&str>) -> Option<String> {
		harness_path(inherited.map(OsStr::new), home.map(Path::new))
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
}
