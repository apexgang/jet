//! Filesystem work the async core does off its runtime.
//!
//! Every filesystem call here is synchronous, so it runs on a blocking
//! thread: a slow mount stalls that thread and not the runtime every
//! connection shares.

use std::io;
use std::path::PathBuf;

use crate::error::CoreError;

/// Runs `work` on a blocking thread and hands its answer back.
///
/// # Errors
///
/// Returns an `internal` [`CoreError`] when the blocking task itself
/// cannot complete, which indicates a programming error.
pub(crate) async fn blocking<T: Send + 'static>(
	work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CoreError> {
	tokio::task::spawn_blocking(work).await.map_err(|error| {
		CoreError::internal("filesystem.task_failed", error.to_string())
	})
}

/// Resolves `path` as the filesystem names it. The error keeps its kind,
/// so a caller can tell a path that does not exist from one it cannot
/// reach without reading native text (ADR-0068).
pub(crate) async fn canonicalize(path: PathBuf) -> io::Result<PathBuf> {
	blocking(move || std::fs::canonicalize(path))
		.await
		.map_err(|error| io::Error::other(error.to_string()))?
}
