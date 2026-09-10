//! Actor-scoped Command receipts: the durable record that makes a retry
//! return the first answer instead of acting twice (ADR-0093).

use jet_store::CommandReceiptRecord;

use crate::command::CommandOutcome;
use crate::error::{ConflictState, CoreError, RecoveryAction};

/// Version of the private encoding a receipt stores its outcome in.
pub(crate) const OUTCOME_VERSION: u32 = 2;

/// Encoding written by the immediately previous rollback-compatible release.
const PREVIOUS_OUTCOME_VERSION: u32 = 1;

/// Schedule results must be rejected safely by the previous name-aware release.
const SCHEDULE_OUTCOME_VERSION: u32 = 3;

/// Utility results must be rejected safely by the previous schedule-aware release.
const UTILITY_OUTCOME_VERSION: u32 = 4;
/// Craft installation receipts are unknown to every previous release.
const CRAFT_INSTALLATION_OUTCOME_VERSION: u32 = 5;
/// Exact remote action reviews require a new receipt vocabulary.
const REMOTE_REVIEW_OUTCOME_VERSION: u32 = 6;
/// Lifecycle controls cannot be replayed by a release that lacks disable barriers.
const CRAFT_LIFECYCLE_OUTCOME_VERSION: u32 = 7;
/// Native extension changes introduced a durable staged result.
const EXTENSION_OUTCOME_VERSION: u32 = 8;
/// Older releases cannot replay an exact-action review retry grant.
const APPROVAL_RETRY_OUTCOME_VERSION: u32 = 9;
/// Older releases cannot replay Auto-continue policy Commands.
const AUTO_CONTINUE_OUTCOME_VERSION: u32 = 10;
const GIT_DELIVERY_OUTCOME_VERSION: u32 = 11;

/// Uses the rollback release's encoding whenever that release can understand
/// the result. Version 2 is reserved for name-only variants introduced here;
/// additive fields on established structs remain valid v1 JSON because the
/// previous serde readers ignore unknown fields (ADR-0073, ADR-0093).
pub(crate) fn outcome_version(
	result: &Result<CommandOutcome, CoreError>,
) -> u32 {
	match result {
		Ok(CommandOutcome::AutoContinueConfigured) => {
			AUTO_CONTINUE_OUTCOME_VERSION
		}
		Ok(CommandOutcome::ApprovalRetryAuthorized { .. }) => {
			APPROVAL_RETRY_OUTCOME_VERSION
		}
		Ok(CommandOutcome::ExtensionChangeQueued { .. }) => {
			EXTENSION_OUTCOME_VERSION
		}
		Ok(CommandOutcome::CraftDisabled { .. }) => {
			CRAFT_LIFECYCLE_OUTCOME_VERSION
		}
		Ok(CommandOutcome::RemoteToolReviewed { .. }) => {
			REMOTE_REVIEW_OUTCOME_VERSION
		}
		Ok(
			CommandOutcome::GitDeliveryQueued { .. }
			| CommandOutcome::GitDeliveryAcknowledged { .. },
		) => GIT_DELIVERY_OUTCOME_VERSION,
		Ok(CommandOutcome::UtilityQueued { .. }) => UTILITY_OUTCOME_VERSION,
		Ok(CommandOutcome::CraftInstallationQueued { .. }) => {
			CRAFT_INSTALLATION_OUTCOME_VERSION
		}
		Ok(
			CommandOutcome::ScheduleCreated(_)
			| CommandOutcome::ScheduleCanceled { .. },
		) => SCHEDULE_OUTCOME_VERSION,
		Ok(
			CommandOutcome::ConversationNamed(_) | CommandOutcome::RunNamed(_),
		) => OUTCOME_VERSION,
		Ok(
			CommandOutcome::UserEditApplied(_)
			| CommandOutcome::Terminal(_)
			| CommandOutcome::TurnWithdrawn(_)
			| CommandOutcome::TurnAdmitted(_)
			| CommandOutcome::ExecutionResolutionRecorded(_)
			| CommandOutcome::RunControlAccepted { .. }
			| CommandOutcome::ConversationCreated(_)
			| CommandOutcome::RunCreated(_)
			| CommandOutcome::RunTransitioned(_)
			| CommandOutcome::SettingSet { .. }
			| CommandOutcome::SettingCleared { .. }
			| CommandOutcome::AccountBound(_)
			| CommandOutcome::PairingGateSet { .. }
			| CommandOutcome::PairingOpened { .. }
			| CommandOutcome::PairingClaimed { .. }
			| CommandOutcome::PairingConfirmed { .. }
			| CommandOutcome::PairingCompleted { .. }
			| CommandOutcome::PairedClientAccessSet { .. }
			| CommandOutcome::PairedClientRevoked { .. }
			| CommandOutcome::AuditEpochBegun { .. }
			| CommandOutcome::AccountUnbound { .. }
			| CommandOutcome::ProjectRegistered(_)
			| CommandOutcome::WorkspacePromotionRecorded(_)
			| CommandOutcome::ConversationImported(_),
		) => PREVIOUS_OUTCOME_VERSION,
		Err(error)
			if matches!(
				error
					.revision_conflict
					.as_ref()
					.map(|conflict| &conflict.safe_state),
				Some(ConflictState::Conversation(_))
			) || error.recovery_actions.iter().any(|action| {
				matches!(action, RecoveryAction::RefreshConversation { .. })
			}) =>
		{
			OUTCOME_VERSION
		}
		Err(_) => PREVIOUS_OUTCOME_VERSION,
	}
}

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
	let Some(outcome) = receipt.outcome else {
		return Err(invalid_receipt("outcome"));
	};
	match outcome_version {
		OUTCOME_VERSION
		| SCHEDULE_OUTCOME_VERSION
		| UTILITY_OUTCOME_VERSION
		| CRAFT_INSTALLATION_OUTCOME_VERSION
		| REMOTE_REVIEW_OUTCOME_VERSION
		| CRAFT_LIFECYCLE_OUTCOME_VERSION
		| EXTENSION_OUTCOME_VERSION
		| APPROVAL_RETRY_OUTCOME_VERSION
		| AUTO_CONTINUE_OUTCOME_VERSION
		| GIT_DELIVERY_OUTCOME_VERSION => decode_result(&outcome),
		PREVIOUS_OUTCOME_VERSION => decode_previous_result(&outcome),
		_ => Ok(Err(CoreError::incompatible(
			"command.outcome_incompatible",
			"the Command outcome was recorded by an incompatible core; retry with that core and do not submit it under a new identity",
		))),
	}
}

fn decode_result(
	outcome: &str,
) -> Result<Result<CommandOutcome, CoreError>, CoreError> {
	serde_json::from_str(outcome).map_err(|error| {
		CoreError::internal("command.outcome_invalid", error.to_string())
	})
}

/// Adds the entity fields introduced in v2 before decoding the previous
/// release's otherwise-compatible result. The original receipt still answers
/// the retry; only deterministic display metadata is synthesized.
fn decode_previous_result(
	outcome: &str,
) -> Result<Result<CommandOutcome, CoreError>, CoreError> {
	let mut value: serde_json::Value =
		serde_json::from_str(outcome).map_err(|error| {
			CoreError::internal("command.outcome_invalid", error.to_string())
		})?;
	if let Some(success) = value.get_mut("Ok") {
		if let Some(conversation) = success.get_mut("ConversationCreated") {
			upgrade_conversation(conversation);
		}
		for kind in ["RunCreated", "RunTransitioned"] {
			if let Some(run) = success.get_mut(kind) {
				upgrade_run(run);
			}
		}
		if let Some(run) = success
			.get_mut("RunControlAccepted")
			.and_then(|accepted| accepted.get_mut("run"))
		{
			upgrade_run(run);
		}
	}
	if let Some(run) = value
		.get_mut("Err")
		.and_then(|error| error.get_mut("revision_conflict"))
		.and_then(|conflict| conflict.get_mut("safe_state"))
		.and_then(|safe_state| safe_state.get_mut("Run"))
	{
		upgrade_run(run);
	}
	serde_json::from_value(value).map_err(|error| {
		CoreError::internal("command.outcome_invalid", error.to_string())
	})
}

fn upgrade_conversation(value: &mut serde_json::Value) {
	let Some(entity) = value.as_object_mut() else {
		return;
	};
	let id = entity
		.get("conversation_id")
		.and_then(serde_json::Value::as_str)
		.and_then(|id| id.parse::<uuid::Uuid>().ok());
	entity
		.entry("revision")
		.or_insert_with(|| serde_json::json!(1));
	if let Some(id) = id {
		entity
			.entry("name")
			.or_insert_with(|| fallback_name("Conversation", id));
	}
}

fn upgrade_run(value: &mut serde_json::Value) {
	let Some(entity) = value.as_object_mut() else {
		return;
	};
	let id = entity
		.get("run_id")
		.and_then(serde_json::Value::as_str)
		.and_then(|id| id.parse::<uuid::Uuid>().ok());
	if let Some(id) = id {
		entity
			.entry("name")
			.or_insert_with(|| fallback_name("Run", id));
	}
}

fn fallback_name(kind: &str, id: uuid::Uuid) -> serde_json::Value {
	serde_json::json!({
		"value": format!("{kind} {}", &id.simple().to_string()[..8]),
		"source": "deterministic",
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

#[cfg(test)]
#[path = "command_receipt_tests.rs"]
mod tests;
