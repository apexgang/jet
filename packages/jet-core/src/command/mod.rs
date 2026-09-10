//! Commands: authenticated, durable mutations. Each one commits its
//! current-state change and its journal Event in one transaction and is
//! acknowledged only after that commit (ADR-0020, ADR-0071).

mod mutation;
pub(crate) use mutation::create_run;
use mutation::{
	clear_setting, create_conversation, set_setting, transition_run,
};

mod dispatch;
use dispatch::{TransactionContext, execute_new};

mod execute;

mod outcome;
pub use outcome::CommandOutcome;

mod request;
pub use request::Command;

pub(crate) mod lifecycle;
pub(crate) mod preparation;
pub(crate) mod receipt;

use crate::error::CoreError;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Actor-scoped identity of a Command, retained for retry safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub Uuid);

/// One Command with the identity and exact request bytes used for retry
/// safety.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvelope {
	/// Actor-scoped identity of the Command.
	command_id: CommandId,
	/// Requested mutation.
	command: Command,
	request_digest: [u8; 32],
}

impl CommandEnvelope {
	/// Builds an envelope and binds its typed Command to a digest of the exact
	/// encoded body. The encoded bytes include unknown compatible fields and
	/// representation differences, so only a byte-equivalent retry reuses the
	/// result.
	///
	/// # Errors
	///
	/// Returns an internal error if the typed Command cannot be encoded for
	/// the binding digest.
	pub fn new(
		command_id: CommandId,
		command: Command,
		request_bytes: &[u8],
	) -> Result<Self, CoreError> {
		let command_bytes = serde_json::to_vec(&command).map_err(|error| {
			CoreError::internal("command.encode_failed", error.to_string())
		})?;
		let mut digest = Sha256::new();
		digest.update(
			u64::try_from(request_bytes.len())
				.unwrap_or(u64::MAX)
				.to_be_bytes(),
		);
		digest.update(request_bytes);
		digest.update(command_bytes);
		Ok(Self {
			command_id,
			command,
			request_digest: digest.finalize().into(),
		})
	}

	#[cfg(test)]
	pub(crate) fn request_digest(&self) -> [u8; 32] {
		self.request_digest
	}
}
