use std::os::unix::fs::symlink;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use crate::error::ErrorCategory;
use crate::relative_path::{GrantedRoot, RelativePath};

#[test]
fn a_relative_path_keeps_the_components_it_was_given() {
	let path = RelativePath::parse("src/main.rs").unwrap();

	assert_eq!(path.as_str(), "src/main.rs");
}

/// ADR-0101 lists what a relative path may not be: absolute, a parent
/// traversal, a NUL, or a form one of the supported platforms would read
/// differently. Each is refused by its own stable code, before anything
/// touches a filesystem.
#[test]
fn a_path_that_does_not_stay_inside_a_root_is_refused() {
	let long_component = "x".repeat(256);
	let refused = [
		"",
		"/etc/passwd",
		"\\\\server\\share",
		"C:\\Users",
		"c:",
		"../secrets",
		"src/../../secrets",
		"src/..",
		"a\0b",
		"src\\main.rs",
		"src//main.rs",
		"src/",
		"./src",
		"src/./main.rs",
		"src/a\tb",
		"src/a\u{7f}b",
		long_component.as_str(),
	]
	.map(|text| {
		let error = RelativePath::parse(text).unwrap_err();
		(error.category, error.code)
	});

	assert_eq!(
		refused,
		[
			(ErrorCategory::InvalidInput, "path.empty".into()),
			(ErrorCategory::InvalidInput, "path.absolute".into()),
			(ErrorCategory::InvalidInput, "path.absolute".into()),
			(ErrorCategory::InvalidInput, "path.absolute".into()),
			(ErrorCategory::InvalidInput, "path.absolute".into()),
			(ErrorCategory::InvalidInput, "path.parent_traversal".into()),
			(ErrorCategory::InvalidInput, "path.parent_traversal".into()),
			(ErrorCategory::InvalidInput, "path.parent_traversal".into()),
			(ErrorCategory::InvalidInput, "path.nul".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.platform_form".into()),
			(ErrorCategory::InvalidInput, "path.too_long".into()),
		]
	);
}

#[test]
fn a_path_longer_than_a_platform_accepts_is_refused_as_a_whole() {
	let deep = std::iter::repeat_n("abcdefghij", 410)
		.collect::<Vec<_>>()
		.join("/");

	let error = RelativePath::parse(&deep).unwrap_err();

	assert_eq!(error.code, "path.too_long");
}

/// A root with a file, a link that stays inside it, a link that leaves it,
/// and a link to nowhere.
struct Root {
	dir: tempfile::TempDir,
	/// The root as the filesystem names it.
	canonical: PathBuf,
}

fn root() -> Root {
	let dir = tempfile::tempdir().unwrap();
	let elsewhere = dir.path().join("elsewhere");
	let root = dir.path().join("root");
	std::fs::create_dir_all(root.join("dir")).unwrap();
	std::fs::create_dir_all(&elsewhere).unwrap();
	std::fs::write(root.join("dir/file.txt"), b"hello").unwrap();
	std::fs::write(elsewhere.join("secret"), b"no").unwrap();
	symlink("dir", root.join("inside")).unwrap();
	symlink(&elsewhere, root.join("outside")).unwrap();
	symlink("nowhere", root.join("gone")).unwrap();
	Root {
		canonical: root.canonicalize().unwrap(),
		dir,
	}
}

fn resolve(root: &Root, text: &str) -> Result<PathBuf, String> {
	RelativePath::parse(text)
		.unwrap()
		.resolve_within(&GrantedRoot::verify(&root.canonical).unwrap())
		.map_err(|error| error.code)
}

/// A grant names one canonical directory (ADR-0101). A root that is now a
/// link to somewhere else, or that is gone, is refused rather than
/// followed, and a root granted through a link was never canonical.
#[test]
fn a_root_is_used_only_while_it_is_still_the_directory_that_was_granted() {
	let root = root();
	let alias = root.dir.path().join("alias");
	symlink(&root.canonical, &alias).unwrap();
	let verified = GrantedRoot::verify(&root.canonical)
		.map(|root| root.path().to_path_buf());
	let through_alias = GrantedRoot::verify(&alias).map_err(|error| error.code);
	std::fs::rename(&root.canonical, root.dir.path().join("moved")).unwrap();
	let gone = GrantedRoot::verify(&root.canonical).map_err(|error| error.code);
	symlink(root.dir.path().join("moved"), &root.canonical).unwrap();
	let moved =
		GrantedRoot::verify(&root.canonical).map_err(|error| error.code);

	assert_eq!(
		(
			verified,
			through_alias.map(|_| ()),
			gone.map(|_| ()),
			moved.map(|_| ())
		),
		(
			Ok(root.canonical.clone()),
			Err("path.root_moved".into()),
			Err("path.root_unreachable".into()),
			Err("path.root_moved".into()),
		)
	);
}

/// A link that stays inside the root resolves to where it points; the
/// components after the last thing that exists are taken as written, since
/// nothing that does not exist can be a link.
#[test]
fn a_path_resolves_under_the_canonical_root() {
	let root = root();

	assert_eq!(
		[
			resolve(&root, "dir/file.txt"),
			resolve(&root, "inside/file.txt"),
			resolve(&root, "dir/new/file.txt"),
			resolve(&root, "inside"),
		],
		[
			Ok(root.canonical.join("dir/file.txt")),
			Ok(root.canonical.join("dir/file.txt")),
			Ok(root.canonical.join("dir/new/file.txt")),
			Ok(root.canonical.join("dir")),
		]
	);
}

/// ADR-0101: symbolic-link resolution that escapes the registered root is
/// refused, whether the link is the whole path or a component of it, and a
/// link that cannot be resolved is refused rather than followed blindly.
#[test]
fn a_link_that_leaves_the_root_is_refused() {
	let root = root();

	assert_eq!(
		[
			resolve(&root, "outside"),
			resolve(&root, "outside/secret"),
			resolve(&root, "outside/new"),
			resolve(&root, "gone"),
			resolve(&root, "gone/deeper"),
		],
		[
			Err("path.escapes_root".into()),
			Err("path.escapes_root".into()),
			Err("path.escapes_root".into()),
			Err("path.link_unresolvable".into()),
			Err("path.link_unresolvable".into()),
		]
	);
}
