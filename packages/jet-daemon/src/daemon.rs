//! Daemon lifecycle: lock, store, listener, serve, drain, shut down.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use jet_core::{Core, WorkspaceHome};
use jet_runtime::{
	DaemonMetadata, InstallationChannel, IpcError, JetHome, LifetimeLock,
	LocalListener, LockError,
};
use jet_store::Store;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

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
) -> ExitCode {
	if let Err(error) = home.prepare() {
		eprintln!(
			"jetd: cannot prepare Jet home {}: {error}",
			home.root().display()
		);
		return ExitCode::from(EXIT_FAILURE);
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
	let core =
		match Core::start(store, WorkspaceHome(home.workspaces_dir())).await {
			Ok(core) => Arc::new(core),
			Err(error) => {
				eprintln!("jetd: cannot start the core: {error}");
				return ExitCode::from(EXIT_FAILURE);
			}
		};
	// A promotion a previous daemon did not finish is settled from what its
	// destination holds before any client can ask for another (ADR-0064,
	// ADR-0067).
	if let Err(error) = core.perform_promotions().await {
		eprintln!("jetd: cannot reconcile Workspace promotions: {error}");
	}
	// ADR-0086: the Plane reports what it can do at startup, on the one
	// line a launcher reads, and on demand afterwards.
	let capabilities = crate::translate::capabilities(
		core.capabilities().await,
		jet_protocol::PROTOCOL_MINOR,
	);
	println!(
		"{}",
		serde_json::json!({
			"status": "ready",
			"socket": listener.socket_path().display().to_string(),
			"capabilities": capabilities,
		})
	);
	let exit = serve(listener, &core).await;
	close_store(&core).await;
	drop(lock);
	exit
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
				Err(IpcError::PeerRejected { uid }) => {
					eprintln!("jetd: refused local connection from uid {uid}");
				}
				Err(error) => {
					eprintln!("jetd: accept failed: {error}");
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
