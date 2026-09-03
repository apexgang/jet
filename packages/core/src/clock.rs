//! Wall clock boundary for retention windows and display timestamps.

use std::time::SystemTime;

/// Supplies wall-clock time to the core.
///
/// Production uses [`SystemClock`]. Tests may provide a controllable clock
/// when behavior depends on a retention window rather than elapsed runtime.
pub trait Clock: std::fmt::Debug + Send + Sync {
	/// Returns the current wall-clock time.
	fn now(&self) -> SystemTime;
}

/// Clock backed by the operating system wall clock.
#[derive(Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
	fn now(&self) -> SystemTime {
		SystemTime::now()
	}
}
