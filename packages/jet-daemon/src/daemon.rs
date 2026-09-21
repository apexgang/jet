//! Daemon lifecycle: lock, store, listener, serve, drain, shut down.

use crate::diagnostics::core_failure;
use jet_core::{Core, WorkspaceHome};
use jet_runtime::{
	DaemonMetadata, DebugLogging, Diagnostic, DiagnosticComponent,
	DiagnosticLog, ExecutableRole, InstallationChannel, IpcError, JetHome,
	LifetimeLock, LocalListener, LockError,
};
use jet_store::Store;
use std::{process::ExitCode, sync::Arc, time::Duration};
use tokio::{
	signal::unix::{SignalKind, signal},
	sync::{Semaphore, watch},
	task::JoinSet,
	time::timeout,
};

const EXIT_FAILURE: u8 = 1;
const EXIT_PLANE_OWNED: u8 = 2;

/// How long draining connections may hold up the exit (ADR-0088).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long closing the store may hold up the exit once no connection is
/// being served.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn run(
	home: JetHome,
	channel: InstallationChannel,
	release_key: Option<ed25519_dalek::VerifyingKey>,
	identity: Option<crate::installation_identity::Identity>,
	debug: DebugLogging,
) -> ExitCode {
	if let Err(error) = home.prepare() {
		eprintln!(
			"jetd: cannot prepare Jet home {}: {error}",
			home.root().display()
		);
		return ExitCode::from(EXIT_FAILURE);
	}
	// A Plane is never refused on its diagnostics' account: without a log
	// the daemon serves exactly as before and says so once (ADR-0061).
	match DiagnosticLog::open(
		&home.diagnostics_dir(),
		ExecutableRole::Daemon,
		debug,
	) {
		Ok(log) => log.install(),
		Err(error) => {
			eprintln!("jetd: serving without a Diagnostic log: {error}")
		}
	}
	let metadata = DaemonMetadata {
		pid: std::process::id(),
		version: env!("CARGO_PKG_VERSION").into(),
		channel,
	};
	let lock = match LifetimeLock::acquire(&home, &metadata) {
		Ok(lock) => lock,
		Err(LockError::Held { owner }) => {
			report_owner(&home, owner.as_ref());
			return ExitCode::from(EXIT_PLANE_OWNED);
		}
		Err(LockError::Io(error)) => {
			eprintln!("jetd: cannot acquire the lifetime lock: {error}");
			return ExitCode::from(EXIT_FAILURE);
		}
	};
	let store = match Store::open(&home.store_path()).await {
		Ok(store) => store,
		Err(error) => {
			eprintln!("jetd: cannot open the Plane store: {error}");
			return ExitCode::from(EXIT_FAILURE);
		}
	};
	let listener = match LocalListener::bind(&home) {
		Ok(listener) => listener,
		Err(error) => {
			eprintln!("jetd: cannot bind the local socket: {error}");
			return ExitCode::from(EXIT_FAILURE);
		}
	};
	// The start is recorded only once the daemon can actually serve.
	let core = match Core::start(store, WorkspaceHome(home.workspaces_dir()))
		.await
	{
		Ok(core) => Arc::new(
			core.with_remote_worker(
				std::env::current_exe().expect("jetd executable path"),
			)
			.with_run_host(Arc::new(crate::run::host::CraftProcesses::new(
				identity,
				home.root().to_path_buf(),
				release_key,
			)))
			.with_extension_host(Arc::new(
				crate::craft::extension_host::Extensions {
					home: home.root().to_path_buf(),
				},
			))
			.with_utility_host(Arc::new(
				crate::craft::utility_host::Utilities {
					home: home.root().to_path_buf(),
				},
			))
			.with_review_host(Arc::new(crate::craft::review_host::Reviews {
				home: home.root().to_path_buf(),
			}))
			.with_terminal_host(Arc::new(crate::terminal::host::Terminals)),
		),
		Err(error) => {
			eprintln!("jetd: cannot start the core: {error}");
			return ExitCode::from(EXIT_FAILURE);
		}
	};
	Diagnostic::info(DiagnosticComponent::Process, "daemon started")
		.identity("version", &metadata.version)
		.count("pid", u64::from(metadata.pid))
		.emit();
	match core.recovery_mode() {
		jet_core::RecoveryMode::Serving => reconcile_at_start(&core).await,
		jet_core::RecoveryMode::ReadOnly(reason) => {
			// ADR-0077: the damaged store is served read-only, exactly as
			// found, until an owner restores a verified snapshot. Nothing
			// reconciles or reconnects before that; the workers below wait
			// for it and run the start-time reconciliation then.
			eprintln!(
				"jetd: the Plane store failed its checks ({reason:?}); serving \
				 read-only Recovery mode until a snapshot is restored"
			);
			Diagnostic::error(
				DiagnosticComponent::Recovery,
				"store failed its checks; serving read-only Recovery mode",
			)
			.failure(&format!("{reason:?}"))
			.emit();
		}
	}
	// ADR-0086: the Plane reports what it can do at startup, on the one
	// line a launcher reads, and on demand afterwards. The line precedes
	// the workers, so the maintenance a start owes never holds it up
	// (ADR-0022).
	let capabilities = crate::translate::capabilities(
		core.capabilities().await,
		jet_protocol::PROTOCOL_MINOR,
	);
	let recovery_mode = match core.recovery_mode() {
		jet_core::RecoveryMode::Serving => "serving",
		jet_core::RecoveryMode::ReadOnly(_) => "read_only",
	};
	println!(
		"{}",
		serde_json::json!({
			"status": "ready",
			"socket": listener.socket_path().display().to_string(),
			"capabilities": capabilities,
			"recovery": recovery_mode,
		})
	);
	let utility_core = Arc::clone(&core);
	let utility_work = tokio::spawn(async move {
		utility_core.wait_until_serving().await;
		loop {
			if let Err(error) = utility_core.perform_git_deliveries().await {
				core_failure(
					DiagnosticComponent::Utility,
					"cannot settle Utility work",
					&error,
				);
				tokio::time::sleep(Duration::from_secs(5)).await;
				continue;
			}
			utility_core.wait_for_utility_work().await;
		}
	});
	let recovery_core = Arc::clone(&core);
	let work_core = Arc::clone(&core);
	let run_work = tokio::spawn(async move {
		work_core.wait_until_serving().await;
		loop {
			work_core.wait_for_run_work().await;
			if let Err(error) = work_core.perform_runs().await {
				core_failure(
					DiagnosticComponent::Run,
					"cannot dispatch queued Run work",
					&error,
				);
			}
		}
	});
	let recovery = tokio::spawn(async move {
		if recovery_core.recovery_mode() != jet_core::RecoveryMode::Serving {
			recovery_core.wait_until_serving().await;
			reconcile_at_start(&recovery_core).await;
		}
		// The day's first Recovery snapshot, then the sweeps it precedes,
		// run once the Plane serves: the copy costs what the store weighs
		// (ADR-0097, ADR-0022). What fails here is owed again next start.
		if let Err(error) = recovery_core.perform_start_maintenance().await {
			core_failure(
				DiagnosticComponent::Maintenance,
				"cannot settle the maintenance a start owes",
				&error,
			);
		}
		loop {
			let mut retry = false;
			if let Err(error) = recovery_core.reconcile_crafts().await {
				retry = true;
				core_failure(
					DiagnosticComponent::Craft,
					"cannot reconcile Crafts",
					&error,
				);
			}

			if let Err(error) =
				recovery_core.perform_craft_installations().await
			{
				retry = true;
				core_failure(
					DiagnosticComponent::Craft,
					"cannot reconcile Craft installations",
					&error,
				);
			}
			if let Err(error) = recovery_core.perform_extension_changes().await
			{
				retry = true;
				core_failure(
					DiagnosticComponent::Extension,
					"cannot apply native extension changes",
					&error,
				);
			}
			if let Err(error) = recovery_core.perform_schedules().await {
				retry = true;
				core_failure(
					DiagnosticComponent::Maintenance,
					"cannot advance schedules",
					&error,
				);
			}
			if let Err(error) = recovery_core.perform_terminals().await {
				retry = true;
				core_failure(
					DiagnosticComponent::Terminal,
					"cannot recover terminals",
					&error,
				);
			}
			if let Err(error) = recovery_core.recover_runs().await {
				retry = true;
				core_failure(
					DiagnosticComponent::Run,
					"cannot recover executions",
					&error,
				);
			}
			if let Err(error) = recovery_core.constrain_child_work().await {
				retry = true;
				core_failure(
					DiagnosticComponent::Run,
					"cannot apply child Energy policy",
					&error,
				);
			}
			// Retention policies stage into Jet Trash and grace periods end
			// on the same wakeups; the sweep is idle when nothing is due
			// (ADR-0015).
			match recovery_core.sweep_retention().await {
				Ok(sweep) => {
					// What was forgotten is the journal's history, not the
					// log's; only the sweep's size is worth a line.
					if !sweep.trashed.is_empty() || !sweep.deleted.is_empty() {
						Diagnostic::info(
							DiagnosticComponent::Maintenance,
							"retention sweep",
						)
						.count("trashed", sweep.trashed.len() as u64)
						.count("deleted", sweep.deleted.len() as u64)
						.emit();
					}
				}
				Err(error) => {
					retry = true;
					core_failure(
						DiagnosticComponent::Maintenance,
						"cannot sweep retention",
						&error,
					);
				}
			}
			// Approved Autodelete rules stage their matches on the same
			// wakeups; drafts and refused compilations select nothing
			// (ADR-0015).
			match recovery_core.sweep_autodelete().await {
				Ok(sweep) => {
					if !sweep.trashed.is_empty() {
						Diagnostic::info(
							DiagnosticComponent::Maintenance,
							"Autodelete sweep",
						)
						.count("trashed", sweep.trashed.len() as u64)
						.emit();
					}
				}
				Err(error) => {
					retry = true;
					core_failure(
						DiagnosticComponent::Maintenance,
						"cannot sweep Autodelete rules",
						&error,
					);
				}
			}
			// Usage rows are recounted into their aggregates and swept past
			// their retention tiers on the same wakeups (ADR-0045).
			match recovery_core.sweep_usage_history().await {
				Ok(sweep) => {
					if !sweep.is_empty() {
						Diagnostic::info(
							DiagnosticComponent::Maintenance,
							"Usage history sweep",
						)
						.count("recounted_hours", sweep.recounted_hours)
						.count("observations", sweep.observations)
						.count("snapshots", sweep.snapshots)
						.count("hours", sweep.hours)
						.emit();
					}
				}
				Err(error) => {
					retry = true;
					core_failure(
						DiagnosticComponent::Maintenance,
						"cannot sweep Usage history",
						&error,
					);
				}
			}
			// Every Command and Effect commit wakes this loop, so the first
			// meaningful change of a day is copied soon after it lands
			// (ADR-0097). A failed copy is retried on the next wake.
			match recovery_core.snapshot_if_due().await {
				Ok(Some(snapshot)) => {
					Diagnostic::info(
						DiagnosticComponent::Recovery,
						"took Recovery snapshot",
					)
					.identity("snapshot", &snapshot.name)
					.emit();
				}
				Ok(None) => {}
				Err(error) => {
					core_failure(
						DiagnosticComponent::Recovery,
						"cannot take the Recovery snapshot",
						&error,
					);
				}
			}
			// Sweep once on startup, including durable extension/install work that
			// has no active Run or schedule to supply the first wakeup.
			if retry {
				tokio::time::sleep(Duration::from_secs(1)).await;
			} else if let Err(error) =
				recovery_core.wait_for_maintenance().await
			{
				core_failure(
					DiagnosticComponent::Maintenance,
					"cannot determine maintenance deadline",
					&error,
				);
				tokio::time::sleep(Duration::from_secs(1)).await;
			}
		}
	});
	let exit = serve(listener, &core).await;
	Diagnostic::info(DiagnosticComponent::Process, "daemon stopping").emit();
	recovery.abort();
	run_work.abort();
	utility_work.abort();
	let _ = utility_work.await;
	close_store(&core).await;
	drop(lock);
	exit
}

/// Settles what a previous daemon left unfinished before new Commands are
/// served, in the order the durable state requires. Each step reports its
/// own failure and the next still runs.
async fn reconcile_at_start(core: &Arc<Core>) {
	if let Err(error) = core.reconcile_crafts().await {
		core_failure(
			DiagnosticComponent::Craft,
			"cannot reconcile Crafts",
			&error,
		);
	}
	// A direct edit that reached its atomic replacement before an interruption
	// is reconciled from its durable intent before clients can retry it.
	if let Err(error) = core.perform_user_edits().await {
		core_failure(
			DiagnosticComponent::Run,
			"cannot reconcile direct user edits",
			&error,
		);
	}
	// A verified Artifact accepted before a restart is published before
	// capabilities or new Commands can observe the installed Craft.
	if let Err(error) = core.perform_craft_installations().await {
		core_failure(
			DiagnosticComponent::Craft,
			"cannot reconcile Craft installations",
			&error,
		);
	}
	// A promotion a previous daemon did not finish is settled from what its
	// destination holds before any client can ask for another (ADR-0064,
	// ADR-0067).
	if let Err(error) = core.perform_promotions().await {
		core_failure(
			DiagnosticComponent::Workspace,
			"cannot reconcile Workspace promotions",
			&error,
		);
	}
	if let Err(error) = core.perform_terminals().await {
		core_failure(
			DiagnosticComponent::Terminal,
			"cannot recover terminals",
			&error,
		);
	}
	// Coalesce offline firings before any pending input can start a Run.
	if let Err(error) = core.perform_schedules().await {
		core_failure(
			DiagnosticComponent::Maintenance,
			"cannot recover schedules",
			&error,
		);
	}
	// Settle durable Run admission before serving new Commands.
	if let Err(error) = core.perform_runs().await {
		core_failure(
			DiagnosticComponent::Run,
			"cannot reconcile Run starts",
			&error,
		);
	}
	// An interrupted escalation is observed, never continued blindly: a
	// signal already delivered may have ended work whose outcome is not yet
	// visible (ADR-0083).
	if let Err(error) = core.perform_run_controls().await {
		core_failure(
			DiagnosticComponent::Run,
			"cannot reconcile execution control",
			&error,
		);
	}
	if let Err(error) = core.recover_runs().await {
		core_failure(
			DiagnosticComponent::Run,
			"cannot recover executions",
			&error,
		);
	}
}

/// Closes the store so SQLite checkpoints its write-ahead log on the way
/// out. An unclosed store loses nothing (ADR-0071), so the exit is never
/// held up for long on its account.
async fn close_store(core: &Core) {
	if timeout(CLOSE_TIMEOUT, core.close()).await.is_err() {
		eprintln!(
			"jetd: the Plane store did not close within {} s; exiting anyway",
			CLOSE_TIMEOUT.as_secs()
		);
	}
}

/// Accepts connections until a stop signal or a listener failure, then
/// drains: the socket closes, every connection finishes the request it is
/// on and is told to reconnect later, and the daemon exits within
/// [`DRAIN_TIMEOUT`] either way (ADR-0088).
async fn serve(listener: LocalListener, core: &Arc<Core>) -> ExitCode {
	let Ok(mut terminate) = signal(SignalKind::terminate()) else {
		eprintln!("jetd: cannot listen for SIGTERM");
		return ExitCode::from(EXIT_FAILURE);
	};
	let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
		eprintln!("jetd: cannot listen for SIGINT");
		return ExitCode::from(EXIT_FAILURE);
	};
	let (drain, draining) = watch::channel(false);
	let mut connections = JoinSet::new();
	let capacity = Arc::new(Semaphore::new(128));
	let exit = loop {
		tokio::select! {
			_ = terminate.recv() => break ExitCode::SUCCESS,
			_ = interrupt.recv() => break ExitCode::SUCCESS,
			Some(_) = connections.join_next(), if !connections.is_empty() => {}
			accepted = listener.accept() => match accepted {
				Ok(stream) => {
					let Ok(permit) = Arc::clone(&capacity).try_acquire_owned() else { continue; };
					let (core, draining) = (Arc::clone(core), draining.clone());
					connections.spawn(async move {
						crate::connection::serve(core, stream, draining, Arc::new(permit)).await;
					});
				}
				Err(IpcError::PeerRejected { .. }) => {
					// ASVS 16.2.5: record the rejection without the peer's identity.
					Diagnostic::warn(
						DiagnosticComponent::Connection,
						"refused local connection from a different user",
					)
					.emit();
				}
				Err(error) => {
					eprintln!("jetd: accept failed: {error}");
					Diagnostic::error(DiagnosticComponent::Connection, "accept failed")
						.failure(&error)
						.emit();
					break ExitCode::from(EXIT_FAILURE);
				}
			},
		}
	};
	drop(listener);
	let _ = drain.send(true);
	let drained = async { while connections.join_next().await.is_some() {} };
	if timeout(DRAIN_TIMEOUT, drained).await.is_err() {
		eprintln!(
			"jetd: {} connection(s) did not drain within {} s; exiting anyway",
			connections.len(),
			DRAIN_TIMEOUT.as_secs()
		);
	}
	exit
}

fn report_owner(home: &JetHome, owner: Option<&DaemonMetadata>) {
	let root = home.root().display();
	match owner {
		Some(DaemonMetadata {
			pid,
			version,
			channel,
		}) => eprintln!(
			"jetd: another jetd already owns the Plane at {root}: pid {pid}, version {version}, channel {channel:?}"
		),
		None => eprintln!(
			"jetd: another jetd already owns the Plane at {root}; its metadata is unreadable"
		),
	}
}
