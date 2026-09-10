//! Platform runtime seams for the Jet core executables (ADR-0051).
//!
//! `jet-runtime` owns the narrow operating-system integrations the daemon
//! needs before any Plane state is touched: the Jet home layout (ADR-0014),
//! the per-Plane lifetime lock (ADR-0003), and the owner-only local IPC
//! listener (ADR-0087).

mod home;
mod ipc;
mod power;
pub use power::{PowerState, observe_power};

pub use home::JetHome;
pub use ipc::lock::{
	DaemonMetadata, InstallationChannel, LifetimeLock, LockError,
};
pub use ipc::{IpcError, LocalListener};
pub use process::execution::{
	execution_boot_identity, execution_digest, execution_process_identity,
	read_execution_file, validate_execution_directory,
};
pub use process::no_visa::NoVisaOperation;

pub use terminal::no_visa_terminal::no_visa_terminal;
pub use terminal::pty::{TerminalPipes, TerminalPty};

pub use terminal::spool::{TerminalReplay, TerminalSpool};

mod process;

mod terminal;
