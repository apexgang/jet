//! How `jetd` writes to its Diagnostic log (ADR-0061).
//!
//! The log itself lives in `jet-runtime`; this is the daemon's one seam
//! onto it, so that a Core failure is always recorded the same way: its
//! stable code as a field, its own explanation scrubbed, and nothing of
//! the request that caused it.

use jet_core::CoreError;
use jet_runtime::{Diagnostic, DiagnosticComponent};

/// Records a Core failure the daemon recovered from or will retry.
pub(crate) fn core_failure(
	component: DiagnosticComponent,
	message: &'static str,
	error: &CoreError,
) {
	Diagnostic::warn(component, message)
		.code(&error.code)
		.failure(error)
		.emit();
}
