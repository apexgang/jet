//! Actor-scoped Command receipts: the durable record that makes a retry
//! return the first answer instead of acting twice (ADR-0093).

use crate::{
	command::CommandOutcome,
	error::{ConflictState, CoreError, RecoveryAction},
};
use jet_store::CommandReceiptRecord;

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
/// Older releases cannot replay a snapshot restoration.
const STORE_RECOVERY_OUTCOME_VERSION: u32 = 12;

/// Uses the rollback release's encoding whenever that release can understand
/// the result. Version 2 is reserved for name-only variants introduced here;
/// additive fields on established structs remain valid v1 JSON because the
/// previous serde readers ignore unknown fields (ADR-0073, ADR-0093).
pub(crate) fn outcome_version(
	result: &Result<CommandOutcome, CoreError>,
) -> u32 {
	match result {
		Ok(CommandOutcome::RecoverySnapshotRestored(_)) => {
			STORE_RECOVERY_OUTCOME_VERSION
		}
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
mod tests {
	use std::time::SystemTime;

	use jet_store::{
		ActorRecord, CommandReceiptRecord, RetentionPolicy, RunLifecycle,
	};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		OUTCOME_VERSION, PREVIOUS_OUTCOME_VERSION, encode_result,
		outcome_version, replay,
	};
	use crate::{
		CommandOutcome, ConflictState, Conversation, ConversationId,
		ConversationOrigin, CoreError, Name, NameSource, Revision,
		RevisionConflict, Run, RunId, WorkingTree,
	};

	const DIGEST: [u8; 32] = [7; 32];

	fn receipt(version: u32, outcome: String) -> CommandReceiptRecord {
		CommandReceiptRecord {
			actor: ActorRecord::InteractiveClient {
				client_id: Uuid::nil(),
			},
			command_id: Uuid::now_v7(),
			request_digest: Some(DIGEST),
			recorded_at_unix_ms: 0,
			outcome_version: Some(version),
			outcome: Some(outcome),
		}
	}

	fn deterministic_name(kind: &str, id: Uuid) -> Name {
		Name {
			value: format!("{kind} {}", &id.simple().to_string()[..8]),
			source: NameSource::Deterministic,
		}
	}

	#[test]
	fn version_one_entity_outcomes_replay_with_their_deterministic_names() {
		let conversation_id = ConversationId(
			Uuid::parse_str("12345678-1234-5678-9234-567812345678").unwrap(),
		);
		let run_id = RunId(
			Uuid::parse_str("87654321-1234-5678-9234-567812345678").unwrap(),
		);
		let conversation = Conversation {
			conversation_id,
			revision: Revision(1),
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTree::NoProject,
			origin: ConversationOrigin::New,
			name: deterministic_name("Conversation", conversation_id.0),
			created_at: SystemTime::UNIX_EPOCH,
		};
		let run = Run {
			run_id,
			conversation_id,
			revision: Revision(3),
			lifecycle: RunLifecycle::Active,
			name: deterministic_name("Run", run_id.0),
			created_at: SystemTime::UNIX_EPOCH,
			ended_at: None,
		};
		let mut old_conversation = serde_json::to_value(Ok::<_, CoreError>(
			CommandOutcome::ConversationCreated(conversation.clone()),
		))
		.unwrap();
		let conversation_payload =
			old_conversation["Ok"]["ConversationCreated"]
				.as_object_mut()
				.unwrap();
		conversation_payload.remove("revision");
		conversation_payload.remove("name");
		let mut old_run = serde_json::to_value(Ok::<_, CoreError>(
			CommandOutcome::RunTransitioned(run.clone()),
		))
		.unwrap();
		old_run["Ok"]["RunTransitioned"]
			.as_object_mut()
			.unwrap()
			.remove("name");

		let replayed_conversation = replay(
			receipt(1, serde_json::to_string(&old_conversation).unwrap()),
			DIGEST,
			1,
		)
		.unwrap();
		let replayed_run = replay(
			receipt(1, serde_json::to_string(&old_run).unwrap()),
			DIGEST,
			1,
		)
		.unwrap();

		assert_eq!(
			(replayed_conversation, replayed_run),
			(
				Ok(CommandOutcome::ConversationCreated(conversation)),
				Ok(CommandOutcome::RunTransitioned(run)),
			)
		);
	}

	#[test]
	fn current_receipts_round_trip_without_compatibility_rewriting() {
		let result = Err(CoreError::invalid_input("test.refused", "refused"));
		let encoded = encode_result(&result).unwrap();

		let replayed =
			replay(receipt(OUTCOME_VERSION, encoded), DIGEST, 1).unwrap();

		assert_eq!(replayed, result);
	}

	#[test]
	fn version_one_revision_conflicts_replay_with_their_deterministic_run_name()
	{
		let conversation_id = ConversationId(Uuid::now_v7());
		let run_id = RunId(
			Uuid::parse_str("87654321-1234-5678-9234-567812345678").unwrap(),
		);
		let run = Run {
			run_id,
			conversation_id,
			revision: Revision(3),
			lifecycle: RunLifecycle::Active,
			name: deterministic_name("Run", run_id.0),
			created_at: SystemTime::UNIX_EPOCH,
			ended_at: None,
		};
		let expected = CoreError::revision_conflict(
			"run.revision_conflict",
			"the Run changed",
			RevisionConflict {
				current_revision: run.revision,
				safe_state: ConflictState::Run(run.clone()),
			},
		);
		let mut previous =
			serde_json::to_value(Err::<CommandOutcome, _>(expected.clone()))
				.unwrap();
		previous["Err"]["revision_conflict"]["safe_state"]["Run"]
			.as_object_mut()
			.unwrap()
			.remove("name");

		let replayed = replay(
			receipt(
				PREVIOUS_OUTCOME_VERSION,
				serde_json::to_string(&previous).unwrap(),
			),
			DIGEST,
			1,
		)
		.unwrap();

		assert_eq!(replayed, Err(expected));
	}

	#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
	enum PreviousCommandOutcome {
		ConversationCreated(PreviousConversation),
	}

	#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
	struct PreviousConversation {
		conversation_id: ConversationId,
		retention: RetentionPolicy,
		working_tree: WorkingTree,
		origin: ConversationOrigin,
		created_at: SystemTime,
	}

	#[test]
	fn established_outcomes_stay_readable_by_the_rollback_release() {
		let conversation_id = ConversationId(Uuid::now_v7());
		let conversation = Conversation {
			conversation_id,
			revision: Revision(1),
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTree::NoProject,
			origin: ConversationOrigin::New,
			name: deterministic_name("Conversation", conversation_id.0),
			created_at: SystemTime::UNIX_EPOCH,
		};
		let result = Ok::<_, CoreError>(CommandOutcome::ConversationCreated(
			conversation.clone(),
		));
		let encoded = encode_result(&result).unwrap();
		let decoded: Result<PreviousCommandOutcome, CoreError> =
			serde_json::from_str(&encoded).unwrap();

		assert_eq!(outcome_version(&result), PREVIOUS_OUTCOME_VERSION);
		assert_eq!(
			decoded,
			Ok(PreviousCommandOutcome::ConversationCreated(
				PreviousConversation {
					conversation_id,
					retention: conversation.retention,
					working_tree: conversation.working_tree,
					origin: conversation.origin,
					created_at: conversation.created_at,
				}
			))
		);
		assert_eq!(
			outcome_version(&Ok(CommandOutcome::ConversationNamed(
				conversation
			))),
			OUTCOME_VERSION
		);
	}
}
