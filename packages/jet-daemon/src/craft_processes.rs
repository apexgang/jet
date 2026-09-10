//! One resident process per digest, bounded crash backoff, and idle retirement.
use crate::run_host::{connect, failed};
use jet_core::{CoreError, PinnedCraft};
use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	process::Stdio,
	time::Duration,
};
use tokio::{
	io::AsyncWriteExt,
	net::UnixStream,
	process::{Child, Command},
	sync::Mutex,
	time::Instant,
};
use uuid::Uuid;

const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Default)]
pub(crate) struct CraftProcesses {
	processes: Mutex<HashMap<String, CraftProcess>>,
	pub(crate) identity: Option<crate::installation_identity::Identity>,
	pub(crate) home: PathBuf,
	pub(crate) supervisor: Option<PathBuf>,
	pub(crate) release_key: Option<ed25519_dalek::VerifyingKey>,
}

#[derive(Debug)]
struct CraftProcess {
	craft_id: String,
	child: Child,
	socket: PathBuf,
	spawned: Instant,
	failures: u32,
	idle_since: Option<Instant>,
}

impl CraftProcesses {
	pub(crate) async fn retirement_delay(&self) -> Option<Duration> {
		self.processes
			.lock()
			.await
			.values()
			.filter_map(|process| process.idle_since)
			.map(|since| {
				(since + IDLE_TIMEOUT).saturating_duration_since(Instant::now())
			})
			.min()
	}
	pub(crate) fn new(
		identity: Option<crate::installation_identity::Identity>,
		home: PathBuf,
		release_key: Option<ed25519_dalek::VerifyingKey>,
	) -> Self {
		Self {
			identity,
			home,
			release_key,
			..Self::default()
		}
	}

	#[expect(
		clippy::await_holding_invalid_type,
		reason = "the gate spans startup so concurrent Runs cannot spawn duplicate Crafts for one digest"
	)]
	pub(crate) async fn connect(
		&self,
		runtime: &Path,
		pin: &PinnedCraft,
	) -> Result<UnixStream, CoreError> {
		let mut processes = self.processes.lock().await;
		let failures = if let Some(process) = processes.get_mut(&pin.sha256) {
			process.idle_since = None;
			if process.child.try_wait().map_err(failed)?.is_none() {
				return connect(&process.socket).await;
			}
			let failures =
				if process.spawned.elapsed() >= Duration::from_secs(60) {
					1
				} else {
					(process.failures + 1).min(3)
				};
			// ASVS 15.2.2: one retry schedule per digest, capped at eight seconds.
			// Core separately bounds unsuccessful recovery attempts per Run.
			tokio::time::sleep(Duration::from_secs(1 << failures)).await;
			let _ = tokio::fs::remove_file(&process.socket).await;
			failures
		} else {
			0
		};
		pin.verify().await?;
		let runtime = tokio::fs::canonicalize(runtime).await.map_err(failed)?;
		let socket =
			runtime.join(format!("c-{}.sock", Uuid::new_v4().simple()));
		// ASVS 1.2.5: the exact accepted executable receives a private endpoint.
		let supervisor = self
			.supervisor
			.clone()
			.map_or_else(std::env::current_exe, Ok)
			.map_err(failed)?;
		let child = Command::new(supervisor)
			.arg("craft-supervisor")
			.arg("--executable")
			.arg(&pin.executable)
			.arg("--digest")
			.arg(&pin.sha256)
			.arg("--socket")
			.arg(&socket)
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			// Dropping closes control stdin; the supervisor then kills and reaps
			// its Craft. Killing the supervisor itself would lose ownership.
			.kill_on_drop(false)
			.spawn()
			.map_err(failed)?;
		processes.insert(
			pin.sha256.clone(),
			CraftProcess {
				craft_id: if pin.id.is_empty() {
					crate::run_craft::Contract::of(pin)?.specification.id
				} else {
					pin.id.clone()
				},
				child,
				socket: socket.clone(),
				spawned: Instant::now(),
				failures,
				idle_since: None,
			},
		);
		connect(&socket).await
	}

	#[expect(
		clippy::await_holding_invalid_type,
		reason = "idle retirement and force stop serialize with process creation"
	)]
	pub(crate) async fn maintain(
		&self,
		active: Vec<PinnedCraft>,
		stopped: Vec<String>,
		force_disabled: Vec<String>,
	) -> Result<(), CoreError> {
		let mut processes = self.processes.lock().await;
		let now = Instant::now();
		let retire: Vec<_> = processes
			.iter_mut()
			.filter_map(|(digest, process)| {
				if stopped.contains(digest)
					|| force_disabled.contains(&process.craft_id)
				{
					return Some((digest.clone(), Retirement::Force));
				}
				if active.iter().any(|pin| pin.sha256 == *digest) {
					process.idle_since = None;
					return None;
				}
				let since = process.idle_since.get_or_insert(now);
				(now.duration_since(*since) >= IDLE_TIMEOUT)
					.then(|| (digest.clone(), Retirement::Idle))
			})
			.collect();
		for (digest, reason) in retire {
			if let Some(process) = processes.get_mut(&digest) {
				process.stop(reason).await?;
			}
			if let Some(process) = processes.remove(&digest) {
				let _ = tokio::fs::remove_file(process.socket).await;
			}
		}
		Ok(())
	}
}

enum Retirement {
	Idle,
	Force,
}

impl CraftProcess {
	async fn stop(&mut self, reason: Retirement) -> Result<(), CoreError> {
		if self.child.try_wait().map_err(failed)?.is_some() {
			return Ok(());
		}
		if let Some(mut control) = self.child.stdin.take() {
			if matches!(reason, Retirement::Idle) {
				let _ = control.write_all(b"T").await;
			}
			drop(control);
		}
		tokio::time::timeout(Duration::from_secs(5), self.child.wait())
			.await
			.map_err(failed)?
			.map(|_| ())
			.map_err(failed)
	}
}

#[cfg(test)]
#[path = "craft_processes_tests.rs"]
mod tests;
