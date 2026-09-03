//! Daemon lifecycle: lock, store, listener, serve, shut down.

use std::process::ExitCode;
use std::sync::Arc;

use jet_core::Core;
use jet_runtime::{
	DaemonMetadata, InstallationChannel, IpcError, JetHome, LifetimeLock,
	LocalListener, LockError,
};
use jet_store::Store;
use tokio::signal::unix::{SignalKind, signal};

const EXIT_FAILURE: u8 = 1;
const EXIT_PLANE_OWNED: u8 = 2;

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
	let store = match Store::open(&home.store_path()) {
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
	let core = match Core::start(store) {
		Ok(core) => Arc::new(core),
		Err(error) => {
			eprintln!("jetd: cannot start the core: {error}");
			return ExitCode::from(EXIT_FAILURE);
		}
	};
	println!(
		"{}",
		serde_json::json!({
			"event": "ready",
			"socket": listener.socket_path().display().to_string(),
		})
	);
	let exit = serve(&listener, &core).await;
	drop(listener);
	drop(lock);
	exit
}

async fn serve(listener: &LocalListener, core: &Arc<Core>) -> ExitCode {
	let Ok(mut terminate) = signal(SignalKind::terminate()) else {
		eprintln!("jetd: cannot listen for SIGTERM");
		return ExitCode::from(EXIT_FAILURE);
	};
	let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
		eprintln!("jetd: cannot listen for SIGINT");
		return ExitCode::from(EXIT_FAILURE);
	};
	loop {
		tokio::select! {
			_ = terminate.recv() => return ExitCode::SUCCESS,
			_ = interrupt.recv() => return ExitCode::SUCCESS,
			accepted = listener.accept() => match accepted {
				Ok(stream) => {
					tokio::spawn(crate::connection::serve(Arc::clone(core), stream));
				}
				Err(IpcError::PeerRejected { uid }) => {
					eprintln!("jetd: refused local connection from uid {uid}");
				}
				Err(error) => {
					eprintln!("jetd: accept failed: {error}");
					return ExitCode::from(EXIT_FAILURE);
				}
			},
		}
	}
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
