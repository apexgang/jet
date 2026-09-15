//! `jetd core`: the per-user lifecycle of a GUI-managed core installation
//! (ADR-0026).
//!
//! A GUI updater stages a verified release payload as an immutable version
//! under `~/.jet/core/versions`, then activates it: the compatibility rule
//! runs first, the owning `jetd` is drained and must relinquish its
//! lifetime lock, and only then does `current` switch. Live `jetfueld`
//! helpers are never touched; each keeps the executable it was started
//! with until it exits (ADR-0088). Rollback is the same switch back to
//! `previous`. The service manager, not this command, starts the daemon
//! that `current` now names. A daemon managed by another channel is
//! drained only when the caller names that channel, because changing
//! channels is an explicit user operation.
//!
//! Every subcommand prints one JSON object on standard output. Exit codes:
//! `0` on success, `3` when the activation is refused by the rule or the
//! channel policy, `4` when the owner did not relinquish the Plane in
//! time, and `1` for any other failure.

mod compatibility;
mod drain;
mod layout;
mod live_helpers;
mod manifest;

use self::{
	compatibility::{Activation, Refusal},
	drain::{DrainError, DrainOutcome},
	layout::CoreVersions,
};
use jet_runtime::{DaemonMetadata, InstallationChannel, JetHome, LockProbe};
use serde::Serialize;
use std::{path::PathBuf, process::ExitCode, time::Duration};

const EXIT_FAILURE: u8 = 1;
const EXIT_REFUSED: u8 = 3;
const EXIT_DRAIN_TIMEOUT: u8 = 4;

/// What `jetd core` was asked to do.
#[derive(clap::Subcommand)]
pub(crate) enum CoreCommand {
	/// Print this build's release identity, for a payload manifest.
	Describe,
	/// Print the staged, current, and previous versions, live helpers, and
	/// the Plane's owner.
	Status {
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
	},
	/// Verify a release payload and copy it into an immutable version.
	Stage {
		/// Directory holding the four executables and `manifest.json`.
		#[arg(long)]
		payload: PathBuf,
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
	},
	/// Drain the owning jetd and point `current` at a staged version.
	Activate {
		/// The staged version to activate.
		#[arg(long)]
		version: String,
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
		/// Also drain a daemon managed by this other channel; changing
		/// channels is an explicit user operation.
		#[arg(long, value_enum)]
		replace_channel: Option<crate::Channel>,
		/// How long the owner may take to relinquish the Plane.
		#[arg(long, default_value_t = 30)]
		drain_timeout_secs: u64,
	},
	/// Drain the owning jetd and point `current` back at `previous`.
	Rollback {
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
		/// Also drain a daemon managed by this other channel.
		#[arg(long, value_enum)]
		replace_channel: Option<crate::Channel>,
		/// How long the owner may take to relinquish the Plane.
		#[arg(long, default_value_t = 30)]
		drain_timeout_secs: u64,
	},
	/// Ask the owning jetd to drain and wait until it relinquished the Plane.
	Drain {
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
		/// How long the owner may take to relinquish the Plane.
		#[arg(long, default_value_t = 30)]
		drain_timeout_secs: u64,
	},
	/// Remove staged versions other than `current` and `previous`.
	Prune {
		/// Jet home directory; defaults to `~/.jet`.
		#[arg(long)]
		home: Option<PathBuf>,
	},
}

/// What this build is, as a payload manifest records it.
#[derive(Debug, Serialize)]
struct ReleaseIdentity {
	version: &'static str,
	protocol_major: u32,
	protocol_minor: u32,
	schema_version: i64,
}

/// The owner a status report names.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Owner {
	Free,
	Held { daemon: Option<DaemonMetadata> },
}

#[derive(Debug, Serialize)]
struct Status {
	status: &'static str,
	current: Option<String>,
	previous: Option<String>,
	staged: Vec<String>,
	live_helpers: Vec<live_helpers::LiveHelper>,
	owner: Owner,
}

#[derive(Debug, Serialize)]
struct Switched {
	status: &'static str,
	version: String,
	previous: Option<String>,
	drained: Option<DaemonMetadata>,
	live_helpers: usize,
}

#[derive(Debug, Serialize)]
struct Refused<'a> {
	status: &'static str,
	#[serde(flatten)]
	reason: &'a Refusal,
}

pub(crate) async fn run(command: CoreCommand) -> ExitCode {
	match command {
		CoreCommand::Describe => {
			print(&ReleaseIdentity {
				version: env!("CARGO_PKG_VERSION"),
				protocol_major: jet_protocol::PROTOCOL_VERSION,
				protocol_minor: jet_protocol::PROTOCOL_MINOR,
				schema_version: jet_store::embedded_schema_version(),
			});
			ExitCode::SUCCESS
		}
		CoreCommand::Status { home } => {
			with_home(home, |home| async move { status(&home) }).await
		}
		CoreCommand::Stage { payload, home } => {
			with_home(home, |home| async move {
				let manifest = CoreVersions::of(&home).stage(&payload)?;
				print(&serde_json::json!({
					"status": "staged",
					"version": manifest.version,
					"target": manifest.target,
				}));
				Ok(ExitCode::SUCCESS)
			})
			.await
		}
		CoreCommand::Activate {
			version,
			home,
			replace_channel,
			drain_timeout_secs,
		} => {
			with_home(home, |home| async move {
				switch(
					&home,
					&version,
					replace_channel.map(InstallationChannel::from),
					Duration::from_secs(drain_timeout_secs),
				)
				.await
			})
			.await
		}
		CoreCommand::Rollback {
			home,
			replace_channel,
			drain_timeout_secs,
		} => {
			with_home(home, |home| async move {
				let Some(previous) = CoreVersions::of(&home).previous()? else {
					print(&serde_json::json!({
						"status": "refused",
						"code": "no_previous_version",
					}));
					return Ok(ExitCode::from(EXIT_REFUSED));
				};
				switch(
					&home,
					&previous,
					replace_channel.map(InstallationChannel::from),
					Duration::from_secs(drain_timeout_secs),
				)
				.await
			})
			.await
		}
		CoreCommand::Drain {
			home,
			drain_timeout_secs,
		} => {
			with_home(home, |home| async move {
				match drain::drain(
					&home,
					Duration::from_secs(drain_timeout_secs),
				)
				.await
				{
					Ok(outcome) => {
						print(&serde_json::json!({
							"status": "drained",
							"daemon": drained(outcome),
						}));
						Ok(ExitCode::SUCCESS)
					}
					Err(error) => Ok(drain_failed(error)),
				}
			})
			.await
		}
		CoreCommand::Prune { home } => {
			with_home(home, |home| async move {
				let removed = CoreVersions::of(&home).prune()?;
				print(&serde_json::json!({
					"status": "pruned",
					"removed": removed,
				}));
				Ok(ExitCode::SUCCESS)
			})
			.await
		}
	}
}

/// Resolves the Jet home, prepares it, and runs `operation`, reporting an
/// error it returns as a failure.
async fn with_home<F, Fut>(home: Option<PathBuf>, operation: F) -> ExitCode
where
	F: FnOnce(JetHome) -> Fut,
	Fut: std::future::Future<
			Output = Result<ExitCode, Box<dyn std::error::Error>>,
		>,
{
	let Some(home) = crate::jet_home(home) else {
		return ExitCode::from(EXIT_FAILURE);
	};
	if let Err(error) = home.prepare() {
		eprintln!(
			"jetd core: cannot prepare Jet home {}: {error}",
			home.root().display()
		);
		return ExitCode::from(EXIT_FAILURE);
	}
	match operation(home).await {
		Ok(code) => code,
		Err(error) => {
			eprintln!("jetd core: {error}");
			ExitCode::from(EXIT_FAILURE)
		}
	}
}

fn status(home: &JetHome) -> Result<ExitCode, Box<dyn std::error::Error>> {
	let layout = CoreVersions::of(home);
	print(&Status {
		status: "ok",
		current: layout.current()?,
		previous: layout.previous()?,
		staged: layout.staged()?,
		live_helpers: live_helpers::live_helpers(home)?,
		owner: match drain::owner(home)? {
			LockProbe::Free => Owner::Free,
			LockProbe::Held(daemon) => Owner::Held { daemon },
		},
	});
	Ok(ExitCode::SUCCESS)
}

/// Activation and rollback: the rule, the channel policy, the drain, and
/// the switch, in that order, so nothing is drained for a switch that would
/// be refused.
async fn switch(
	home: &JetHome,
	version: &str,
	replace_channel: Option<InstallationChannel>,
	drain_timeout: Duration,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
	let layout = CoreVersions::of(home);
	let candidate = layout
		.manifest(version)
		.map_err(|error| format!("version {version} is not staged: {error}"))?;
	let current = layout.current()?;
	let current_manifest = current
		.as_deref()
		.map(|name| layout.manifest(name))
		.transpose()?;
	let previous = layout.previous()?;
	let store_schema =
		jet_store::applied_schema_version(&home.store_path()).await?;
	let live_helpers = live_helpers::live_helpers(home)?;
	if let Err(reason) = compatibility::check(&Activation {
		candidate: &candidate,
		current: current_manifest.as_ref(),
		invoker_major: jet_protocol::PROTOCOL_VERSION,
		previous: previous.as_deref(),
		store_schema,
		live_helpers: live_helpers.len(),
	}) {
		print(&Refused {
			status: "refused",
			reason: &reason,
		});
		return Ok(ExitCode::from(EXIT_REFUSED));
	}
	if let LockProbe::Held(Some(owner)) = drain::owner(home)?
		&& owner.channel != InstallationChannel::Gui
		&& replace_channel != Some(owner.channel)
	{
		print(&serde_json::json!({
			"status": "refused",
			"code": "channel_owned",
			"owner": owner,
		}));
		return Ok(ExitCode::from(EXIT_REFUSED));
	}
	let outcome = match drain::drain(home, drain_timeout).await {
		Ok(outcome) => outcome,
		Err(error) => return Ok(drain_failed(error)),
	};
	layout.activate(version)?;
	print(&Switched {
		status: "activated",
		version: version.to_owned(),
		previous: layout.previous()?,
		drained: drained(outcome),
		live_helpers: live_helpers.len(),
	});
	Ok(ExitCode::SUCCESS)
}

fn drained(outcome: DrainOutcome) -> Option<DaemonMetadata> {
	match outcome {
		DrainOutcome::Idle => None,
		DrainOutcome::Drained(owner) => Some(owner),
	}
}

fn drain_failed(error: DrainError) -> ExitCode {
	match error {
		DrainError::Timeout { owner, timeout } => {
			print(&serde_json::json!({
				"status": "drain_timeout",
				"owner": owner,
				"timeout_secs": timeout.as_secs(),
			}));
			ExitCode::from(EXIT_DRAIN_TIMEOUT)
		}
		DrainError::OwnerUnknown => {
			print(&serde_json::json!({
				"status": "refused",
				"code": "owner_unknown",
			}));
			ExitCode::from(EXIT_REFUSED)
		}
		DrainError::Lock(_) | DrainError::Signal { .. } => {
			eprintln!("jetd core: {error}");
			ExitCode::from(EXIT_FAILURE)
		}
	}
}

fn print<T: Serialize>(value: &T) {
	println!(
		"{}",
		serde_json::to_string(value).expect("report serializes")
	);
}
