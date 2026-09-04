//! Helpers shared by the core's test modules.

use std::path::Path;

use jet_store::Store;
use uuid::Uuid;

use crate::{Actor, ClientId, Core};

/// The one interactive Actor every core test acts as.
pub(crate) fn actor() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::nil()),
	}
}

/// Starts a core over a fresh or existing store at `path`.
pub(crate) fn start_core(path: &Path) -> Core {
	Core::start(Store::open(path).unwrap()).unwrap()
}
