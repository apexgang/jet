//! Conversation, Run, and Setting mutations inside a Command transaction.

use super::{CommandId, CommandOutcome, lifecycle};
use crate::{
	Actor,
	audit::{self, AuditSubject, Decision},
	conversation::{
		Conversation, ConversationId, ConversationOrigin, Revision, Run, RunId,
	},
	error::{ConflictState, CoreError, RevisionConflict},
	event::{EventKind, EventSubject},
	setting::{self, SettingKey, SettingScope, SettingValue},
	workspace::{self, WorkingTree},
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewConversation, NewEffect, NewRun,
	RetentionPolicy, RunLifecycle, SettingRecord, WorkingTreeRecord,
	WriteTransaction,
};
use uuid::Uuid;

pub(super) async fn create_conversation(
	tx: &mut WriteTransaction,
	actor: &Actor,
	retention: RetentionPolicy,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
			working_tree: WorkingTreeRecord::NoProject,
			origin: ConversationOrigin::New.record(),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationCreated {
		name: Some(conversation.name.clone()),
		retention,
		working_tree: WorkingTree::NoProject,
		origin: ConversationOrigin::New,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(conversation.conversation_id),
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

pub(crate) async fn create_run(
	tx: &mut WriteTransaction,
	actor: &Actor,
	conversation_id: ConversationId,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let Some(conversation) = tx.conversation(conversation_id.0).await? else {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	};
	if lifecycle::any_live(&tx.runs(conversation_id.0).await?) {
		return Err(CoreError::conflict(
			"run.conversation_busy",
			"the Conversation already has a Run that has not ended",
		));
	}
	match WorkingTree::from(conversation.working_tree) {
		WorkingTree::LocalCheckout { project_id } => {
			workspace::admit_local_checkout_run(tx, project_id).await?;
		}
		WorkingTree::NoProject | WorkingTree::Workspace { .. } => {}
	}
	let run: Run = tx
		.insert_run(NewRun {
			run_id: Uuid::now_v7(),
			conversation_id: conversation_id.0,
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	tx.append_event(
		EventKind::RunCreated {
			name: Some(run.name.clone()),
		}
		.to_record(
			actor,
			EventSubject::Run {
				conversation_id,
				run_id: run.run_id,
			},
			now_unix_ms,
		)?,
	)
	.await?;
	Ok(CommandOutcome::RunCreated(run))
}

pub(super) async fn set_setting(
	tx: &mut WriteTransaction,
	actor: &Actor,
	key: SettingKey,
	scope: SettingScope,
	value: SettingValue,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let encoded = setting::prepare_write(key, scope, &value)?;
	setting::require_subject(tx, scope).await?;
	tx.upsert_setting(&SettingRecord {
		key: key.as_str().into(),
		scope: scope.record(),
		value: encoded,
		updated_at_unix_ms: now_unix_ms,
	})
	.await?;
	let event = EventKind::SettingChanged {
		key,
		scope,
		value: value.clone(),
	};
	tx.append_event(event.to_record(
		actor,
		setting::event_subject(scope),
		now_unix_ms,
	)?)
	.await?;
	// ASVS 16.2.1: a policy that decides what Jet may do on its own is
	// recorded in the Security audit as well as in the journal (ADR-0105).
	if let Some(decision) = audit::stored_setting(key, &value) {
		audit::record(
			tx,
			actor,
			Decision::succeeded(decision, AuditSubject::of_scope(scope)),
			now_unix_ms,
		)
		.await?;
	}
	Ok(CommandOutcome::SettingSet { key, scope, value })
}

pub(super) async fn clear_setting(
	tx: &mut WriteTransaction,
	actor: &Actor,
	key: SettingKey,
	scope: SettingScope,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	setting::prepare_clear(key, scope)?;
	setting::require_subject(tx, scope).await?;
	tx.delete_setting(key.as_str(), scope.record()).await?;
	let event = EventKind::SettingCleared { key, scope };
	tx.append_event(event.to_record(
		actor,
		setting::event_subject(scope),
		now_unix_ms,
	)?)
	.await?;
	if let Some(decision) = audit::cleared_setting(key) {
		audit::record(
			tx,
			actor,
			Decision::succeeded(decision, AuditSubject::of_scope(scope)),
			now_unix_ms,
		)
		.await?;
	}
	Ok(CommandOutcome::SettingCleared { key, scope })
}

pub(super) async fn transition_run(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	run_id: RunId,
	expected_revision: Revision,
	lifecycle: RunLifecycle,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	// ASVS 2.3.1: only supervised process observations may change a managed
	// Run's lifecycle. A client cannot release its working-tree exclusion.
	if tx.run_execution(run_id.0).await?.is_some() {
		return Err(CoreError::conflict(
			"run.managed_lifecycle",
			"managed Run lifecycle is owned by execution supervision",
		));
	}
	let Some(current) = tx.run(run_id.0).await? else {
		return Err(CoreError::not_found(
			"run.not_found",
			"the Run does not exist",
		));
	};
	if current.revision != expected_revision.0 {
		let current: Run = current.into();
		return Err(CoreError::revision_conflict(
			"run.revision_conflict",
			"the Run changed since the Command was prepared",
			RevisionConflict {
				current_revision: current.revision,
				safe_state: ConflictState::Run(current),
			},
		));
	}
	if !lifecycle::may_transition(current.lifecycle, lifecycle) {
		return Err(CoreError::conflict(
			"run.invalid_transition",
			format!(
				"a {} Run cannot move to {}",
				current.lifecycle.as_str(),
				lifecycle.as_str()
			),
		));
	}
	let run: Run = tx
		.update_run_lifecycle(run_id.0, lifecycle, now_unix_ms)
		.await?
		.into();
	let event = EventKind::RunLifecycleChanged {
		from: current.lifecycle,
		to: lifecycle,
	};
	tx.append_event(event.to_record(
		actor,
		EventSubject::Run {
			conversation_id: run.conversation_id,
			run_id,
		},
		now_unix_ms,
	)?)
	.await?;
	if lifecycle == RunLifecycle::Starting {
		let effect_id = Uuid::now_v7();
		// ASVS 2.3.3: the Effect is inserted in the same authoritative
		// transaction as the Run state and Event before acknowledgement.
		tx.insert_effect(&NewEffect {
			effect_id,
			command_id: command_id.0,
			run_id: Some(run_id.0),
			promotion_id: None,
			terminal_id: None,
			kind: EffectKindRecord::StartRun,
			safety: EffectSafetyRecord::Idempotent {
				external_key: effect_id,
				max_attempts: 3,
			},
		})
		.await?;
	}
	Ok(CommandOutcome::RunTransitioned(run))
}
