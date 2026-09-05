//! Helpers shared by the core's test modules.

use std::path::Path;

use jet_store::Store;
use uuid::Uuid;

use crate::{Actor, ClientId, Command, CommandEnvelope, CommandId, Core};

/// The one interactive Actor every core test acts as.
pub(crate) fn actor() -> Actor {
	Actor::InteractiveClient {
		client_id: ClientId(Uuid::nil()),
	}
}

/// Starts a core over a fresh or existing store at `path`.
pub(crate) async fn start_core(path: &Path) -> Core {
	Core::start(Store::open(path).await.unwrap()).await.unwrap()
}

/// A fresh Command identity for a request that is not a retry.
pub(crate) fn command_id() -> CommandId {
	CommandId(Uuid::now_v7())
}

/// Wraps `command` in an envelope bound to its own encoded bytes, the way
/// `jetd` binds the bytes a client sent.
pub(crate) fn request(command: Command) -> CommandEnvelope {
	request_with_id(command_id(), command)
}

/// The same envelope under a chosen identity, so a test can retry one
/// Command exactly.
pub(crate) fn request_with_id(
	command_id: CommandId,
	command: Command,
) -> CommandEnvelope {
	let bytes = serde_json::to_vec(&command).unwrap();
	CommandEnvelope::new(command_id, command, &bytes).unwrap()
}
