use pretty_assertions::assert_eq;

use super::{first_line, run_version};
use crate::capability::ToolAvailability;

/// A tool reports its version through whichever stream it prefers, and a
/// version line the Plane cannot bound would grow every snapshot that
/// carries it.
#[test]
fn a_version_line_is_the_first_non_empty_line_and_is_bounded() {
	let git = first_line(b"git version 2.51.0\n");
	let padded = first_line(b"\n   \nOpenSSH_9.9p2, OpenSSL 3.5.4\nmore\n");
	let talkative = first_line(&vec![b'x'; 1_000]);
	let silent = first_line(b"   \n\n");

	assert_eq!(
		(git, padded, talkative.map(|line| line.len()), silent),
		(
			Some("git version 2.51.0".into()),
			Some("OpenSSH_9.9p2, OpenSSL 3.5.4".into()),
			Some(120),
			None
		)
	);
}

/// A Plane that does not have a tool must report it as missing rather than
/// failing to observe itself at all.
#[tokio::test]
async fn a_program_that_cannot_be_run_is_missing() {
	let absent =
		run_version("jet-tool-no-plane-installs", &["--version"]).await;

	assert_eq!(absent, ToolAvailability::Missing);
}
