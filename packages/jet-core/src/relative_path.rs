//! Relative paths inside a registered root (ADR-0101).
//!
//! An ordinary file Command or Query names a Project or Workspace and a
//! path relative to its root; it never names an absolute path. The path is
//! checked as text before anything touches a filesystem, and resolved
//! against a [`GrantedRoot`] afterwards so that a symbolic link inside the
//! root cannot lead outside it. Canonical absolute paths enter the core
//! only through an explicit Path grant, and a root is used only while the
//! filesystem still names the directory that was granted.
//!
//! A path travels between Planes, so it is spelled one way: `/` between
//! components, no empty, current-directory, or parent components, no
//! control characters, and no backslash, which another platform reads as a
//! separator. Case folding and Unicode normalization on macOS are
//! properties of the filesystem that resolution meets, not forms this
//! check rewrites.
//!
//! Resolution is a sequence of path-based system calls, each of which the
//! kernel resolves from `/` again, so it is not atomic: a directory swapped
//! for a link while the walk is under way is followed by the next call. The
//! Plane runs as one user, and code running as that user is outside the
//! threat model (ADR-0105), so the check is a guard against a wrong path
//! and a stale root rather than against a racing process. An fd-relative
//! walk would close that window and is a possible hardening.
//!
//! Everything here reads the filesystem synchronously. A caller in the
//! async core runs it inside `tokio::task::spawn_blocking`, so a slow mount
//! stalls one blocking thread rather than the runtime.

use std::io;
use std::path::{Path, PathBuf};

use rustix::io::Errno;

use crate::error::CoreError;

/// Longest single component both supported platforms accept.
const MAX_COMPONENT_BYTES: usize = 255;

/// Longest relative path accepted as text. The root it is joined to adds
/// its own length, and macOS stops earlier than Linux, so the platform's
/// own answer when the joined path is too long is reported as too long as
/// well.
const MAX_PATH_BYTES: usize = 4096;

/// A validated path relative to a registered root, with `/` between its
/// components. It can be built only by [`RelativePath::parse`], so a value
/// of this type has already been refused if it was absolute, traversed to a
/// parent, held a NUL, or held a backslash or a control character.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelativePath {
	text: String,
}

/// The root of a registered Project or Workspace, as the filesystem names
/// it right now, verified to still be the directory that was granted.
///
/// A Path grant authorizes one canonical absolute path (ADR-0101). If that
/// path has since become a link to somewhere else, following it would widen
/// the grant to a directory nobody granted, so the root is refused instead
/// of resolved. A Workspace mints one of these the same way once Workspaces
/// exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantedRoot(PathBuf);

impl GrantedRoot {
	/// Verifies that `granted`, the canonical path recorded when the grant
	/// was made, still names that directory.
	///
	/// # Errors
	///
	/// Returns an `unavailable` `path.root_unreachable` when the root cannot
	/// be reached, and a `conflict` `path.root_moved` when the filesystem
	/// now resolves it somewhere else.
	pub(crate) fn verify(granted: &Path) -> Result<Self, CoreError> {
		let now = std::fs::canonicalize(granted).map_err(|error| {
			CoreError::unavailable(
				"path.root_unreachable",
				"the registered root cannot be reached on this Plane",
				error.to_string(),
			)
		})?;
		if now != granted {
			return Err(CoreError::conflict(
				"path.root_moved",
				"the registered root now resolves to a different directory; \
				 register the Project again where it lives now",
			));
		}
		Ok(Self(now))
	}

	/// The canonical root.
	pub(crate) fn path(&self) -> &Path {
		&self.0
	}
}

impl RelativePath {
	/// Checks `text` as a relative path without touching any filesystem.
	///
	/// # Errors
	///
	/// Returns an `invalid_input` [`CoreError`] whose code says what was
	/// wrong: `path.empty`, `path.nul`, `path.absolute`,
	/// `path.parent_traversal`, `path.platform_form`, or `path.too_long`.
	pub fn parse(text: &str) -> Result<Self, CoreError> {
		if text.is_empty() {
			return Err(CoreError::invalid_input(
				"path.empty",
				"a path names at least one component",
			));
		}
		if text.contains('\0') {
			return Err(CoreError::invalid_input(
				"path.nul",
				"a path holds no NUL character",
			));
		}
		if is_absolute_form(text) {
			return Err(CoreError::invalid_input(
				"path.absolute",
				"a path is relative to the root it is addressed through, \
				 not absolute",
			));
		}
		if text.len() > MAX_PATH_BYTES {
			return Err(too_long());
		}
		for component in text.split('/') {
			require_component(component)?;
		}
		Ok(Self { text: text.into() })
	}

	/// The path as it was given, with `/` between its components.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.text
	}

	/// Resolves this path under `root`, following symbolic links only
	/// while they stay inside the root.
	///
	/// Each existing component is examined in turn: a link is canonicalized
	/// and must still lie under the root. Once a component does not exist,
	/// the rest of the path is taken as written, because nothing that does
	/// not exist can be a link.
	///
	/// The answer describes the filesystem as the walk read it. A link
	/// replaced between this check and the operation that follows is
	/// outside what any check can promise; callers that mutate keep the
	/// window short by resolving immediately before they act.
	///
	/// # Errors
	///
	/// Returns `path.escapes_root` when a link leads outside the root,
	/// `path.link_unresolvable` when a link leads nowhere or loops,
	/// `path.too_long` when the joined path is longer than the platform
	/// accepts, and an `unavailable` `path.unreadable` when a component
	/// cannot be examined.
	pub(crate) fn resolve_within(
		&self,
		root: &GrantedRoot,
	) -> Result<PathBuf, CoreError> {
		let root = root.path();
		// PATH_MAX counts the terminating NUL, so a joined path of exactly
		// the bound is already too long.
		if root.as_os_str().len() + 1 + self.text.len() >= MAX_PATH_BYTES {
			return Err(too_long());
		}
		let mut current = root.to_path_buf();
		let mut components = self.text.split('/');
		while let Some(component) = components.next() {
			let candidate = current.join(component);
			match std::fs::symlink_metadata(&candidate) {
				Ok(metadata) if metadata.file_type().is_symlink() => {
					let target = std::fs::canonicalize(&candidate)
						.map_err(|error| link_error(component, &error))?;
					if !target.starts_with(root) {
						return Err(CoreError::invalid_input(
							"path.escapes_root",
							format!(
								"the link at {component:?} leads outside the \
								 registered root"
							),
						));
					}
					current = target;
				}
				Ok(_) => current = candidate,
				Err(error)
					if matches!(
						error.kind(),
						io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
					) =>
				{
					// Nothing below here exists, so nothing below here can
					// be a link.
					let mut rest = candidate;
					rest.extend(components);
					return Ok(rest);
				}
				Err(error) => return Err(unreadable(component, &error)),
			}
		}
		Ok(current)
	}
}

/// Whether `text` is absolute on a supported platform, or begins with the
/// separator another platform would read as one.
fn is_absolute_form(text: &str) -> bool {
	text.starts_with('/') || text.starts_with('\\')
}

/// Why a link at `component` could not be followed. A link that leads
/// nowhere or loops is the path's fault; anything else is the Plane's, and
/// the native text stays in the local detail (ADR-0068).
fn link_error(component: &str, error: &io::Error) -> CoreError {
	// A loop has no stable `io::ErrorKind` yet, so it is told by its errno.
	let looped = error.raw_os_error() == Some(Errno::LOOP.raw_os_error());
	if looped
		|| matches!(
			error.kind(),
			io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
		) {
		return CoreError::invalid_input(
			"path.link_unresolvable",
			format!(
				"the link at {component:?} leads nowhere it can be followed"
			),
		);
	}
	unreadable(component, error)
}

/// Why `component` could not be examined: a joined path the platform finds
/// too long is the path's fault; anything else is the Plane's.
fn unreadable(component: &str, error: &io::Error) -> CoreError {
	if error.kind() == io::ErrorKind::InvalidFilename {
		return too_long();
	}
	CoreError::unavailable(
		"path.unreadable",
		format!("the component {component:?} cannot be examined"),
		error.to_string(),
	)
}

/// Refuses a component that is empty, names the current or parent
/// directory, is too long, or holds a backslash or a control character.
fn require_component(component: &str) -> Result<(), CoreError> {
	match component {
		"" => Err(CoreError::invalid_input(
			"path.platform_form",
			"a path has no empty component, leading separator, or trailing \
			 separator",
		)),
		"." => Err(CoreError::invalid_input(
			"path.platform_form",
			"a path names no current-directory component",
		)),
		".." => Err(CoreError::invalid_input(
			"path.parent_traversal",
			"a path does not traverse to a parent directory",
		)),
		_ if component.len() > MAX_COMPONENT_BYTES => Err(too_long()),
		_ if component
			.chars()
			.any(|character| character == '\\' || character.is_control()) =>
		{
			Err(CoreError::invalid_input(
				"path.platform_form",
				"a path component holds no backslash or control character",
			))
		}
		_ => Ok(()),
	}
}

fn too_long() -> CoreError {
	CoreError::invalid_input(
		"path.too_long",
		format!(
			"a path is at most {MAX_PATH_BYTES} bytes with components of at \
			 most {MAX_COMPONENT_BYTES} bytes"
		),
	)
}

#[cfg(test)]
#[path = "relative_path_tests.rs"]
mod tests;
