//! Platform runtime seams for the Jet core executables (ADR-0051).
//!
//! `jet-runtime` owns the narrow operating-system integrations the daemon
//! needs before any Plane state is touched: the Jet home layout (ADR-0014),
//! the per-Plane lifetime lock (ADR-0003), and the owner-only local IPC
//! listener (ADR-0087).

mod home;
mod ipc;
mod lock;

pub use home::JetHome;
pub use ipc::{IpcError, LocalListener};
pub use lock::{DaemonMetadata, InstallationChannel, LifetimeLock, LockError};
