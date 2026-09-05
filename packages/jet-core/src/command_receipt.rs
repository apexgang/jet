//! Actor-scoped Command receipts: the durable record that makes a retry
//! return the first answer instead of acting twice (ADR-0093).

use jet_store::CommandReceiptRecord;

use crate::command::CommandOutcome;
use crate::error::CoreError;

/// Version of the private encoding a receipt stores its outcome in.
pub(crate) const OUTCOME_VERSION: u32 = 1;

/// How long a Command identity keeps its digest and outcome. After it, the
/// identity remains as an expiry tombstone and the Command must be
/// submitted again under a new one.
pub(crate) const COMMAND_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Answers a Command whose identity this Actor already used.
pub(crate) fn replay(
	receipt: CommandReceiptRecord,
	request_digest: [u8; 32],
	now_unix_ms: i64,
) -> Result<Result<CommandOutcome, CoreError>, CoreError> {
	if now_unix_ms.saturating_sub(receipt.recorded_at_unix_ms)
		> COMMAND_RETENTION_MS
	{
		return Ok(Err(CoreError::invalid_input(
			"command.identity_expired",
			"the Command identity is older than thirty days",
		)));
	}
	let Some(original_digest) = receipt.request_digest else {
		return Err(invalid_receipt("digest"));
	};
	if original_digest != request_digest {
		return Err(CoreError::conflict(
			"command.identity_reused",
			"the Command identity was already used for different content",
		));
	}
	let Some(outcome_version) = receipt.outcome_version else {
		return Err(invalid_receipt("outcome version"));
	};
	if outcome_version != OUTCOME_VERSION {
		return Ok(Err(CoreError::incompatible(
			"command.outcome_incompatible",
			"the Command outcome was recorded by an incompatible core; submit the Command again under a new identity",
		)));
	}
	let Some(outcome) = receipt.outcome else {
		return Err(invalid_receipt("outcome"));
	};
	serde_json::from_str(&outcome).map_err(|error| {
		CoreError::internal("command.outcome_invalid", error.to_string())
	})
}

fn invalid_receipt(missing: &str) -> CoreError {
	CoreError::internal(
		"command.receipt_invalid",
		format!("an unexpired Command receipt has no {missing}"),
	)
}

/// Encodes the authoritative result a receipt carries.
pub(crate) fn encode_result(
	result: &Result<CommandOutcome, CoreError>,
) -> Result<String, CoreError> {
	serde_json::to_string(result).map_err(|error| {
		CoreError::internal("command.outcome_encode_failed", error.to_string())
	})
}
