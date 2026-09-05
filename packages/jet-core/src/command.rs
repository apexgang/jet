//! Commands: authenticated, durable mutations. Each one commits its
//! current-state change and its journal Event in one transaction and is
//! acknowledged only after that commit (ADR-0020, ADR-0071).

use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewCommandReceipt, NewConversation,
	NewEffect, NewRun, PairingGate, PairingMethod, RetentionPolicy,
	RunLifecycle, SettingRecord, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::account::{
	self, AccountBinding, AccountBindingId, CredentialReference,
	CredentialSource, ProviderAccount, ProviderId,
};
use crate::audit::{self, AuditEpoch, AuditSubject, Decision};
use crate::capability::{Capability, ExternalTool};
use crate::command_receipt::{
	COMMAND_RETENTION_MS, OUTCOME_VERSION, encode_result, replay,
};
use crate::conversation::{Conversation, ConversationId, Revision, Run, RunId};
use crate::error::{ConflictState, CoreError, RevisionConflict};
use crate::event::{EventKind, EventSubject};
use crate::pairing::{
	self, ClientPublicKey, PairingChallenge, PairingDisclosure, PairingSecret,
	PendingPairing,
};
use crate::pairing_offer;
use crate::security::{self, SecurityClass, SecurityState};
use crate::setting::{self, SettingKey, SettingScope, SettingValue};
use crate::{Actor, Core, lifecycle};

/// Automatic Git delivery is carried out with the Git the core invokes, so
/// turning it on depends on that tool being installed (ADR-0029, ADR-0056).
const GIT: &[Capability] = &[Capability::ExternalTool(ExternalTool::Git)];

/// A binding that resolves through the platform credential store depends on
/// there being one. Jet keeps no secret of its own instead, so a Plane
/// without a store refuses the binding rather than falling back to
/// plaintext (ADR-0076).
const CREDENTIAL_STORE: &[Capability] = &[Capability::CredentialStore];

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
}

/// A state-changing request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Command {
	/// Create a Conversation with no Runs.
	CreateConversation {
		/// Whether Jet keeps the Conversation after its final Run.
		retention: RetentionPolicy,
	},
	/// Record a new Run of a Conversation that has no live Run.
	CreateRun {
		/// The Conversation to execute.
		conversation_id: ConversationId,
	},
	/// Store a Setting value at one scope (ADR-0085).
	SetSetting {
		/// The Setting to store.
		key: SettingKey,
		/// The scope that stores the value.
		scope: SettingScope,
		/// The value to store.
		value: SettingValue,
	},
	/// Remove whatever value one scope stores for a Setting, leaving the
	/// scopes above it untouched.
	ClearSetting {
		/// The Setting to clear.
		key: SettingKey,
		/// The scope that stops storing a value.
		scope: SettingScope,
	},
	/// Bind a Provider account to this Plane, storing only non-secret
	/// metadata and an opaque Credential reference (ADR-0016, ADR-0076).
	BindAccount {
		/// The Provider the binding authenticates to.
		provider: ProviderId,
		/// The user-facing name of the binding.
		label: String,
		/// The Provider's own account identity, when it supplies one.
		provider_account: Option<ProviderAccount>,
		/// The backend that resolves the binding's Credential.
		credential_source: CredentialSource,
	},
	/// Remove an Account binding from this Plane. The secret it referred to
	/// belongs to its backend, so Jet forgets the reference and leaves the
	/// backend to the client that wrote it.
	UnbindAccount {
		/// The binding to remove.
		binding_id: AccountBindingId,
	},
	/// Begin a new authority epoch of the Security audit, carrying on past
	/// an integrity failure and recording the gap it leaves (ADR-0105).
	BeginAuditEpoch,
	/// Open or close this Plane's Pairing gate, which decides whether a new
	/// GUI client may begin Pairing at all (ADR-0017). It does not alter the
	/// clients that are already Paired.
	SetPairingGate {
		/// Where to leave the gate.
		gate: PairingGate,
	},
	/// Issue this Plane's one Pairing offer, replacing whatever it had
	/// open, and disclose its one-time secret to the owner who asked for it
	/// (ADR-0017).
	OpenPairing {
		/// How the secret reaches the person pairing.
		method: PairingMethod,
	},
	/// Claim the open Pairing offer with the secret a person presented and
	/// the public key of the Client identity presenting it.
	ClaimPairing {
		/// The secret as it was presented.
		secret: PairingSecret,
		/// The durable public key that becomes the credential once Pairing
		/// completes.
		key: ClientPublicKey,
	},
	/// Move a Run forward through its lifecycle.
	TransitionRun {
		/// The Run to move.
		run_id: RunId,
		/// Revision observed when the Command was prepared.
		expected_revision: Revision,
		/// The state to enter.
		lifecycle: RunLifecycle,
	},
}

impl Command {
	/// What the Plane must still be able to do when this Command runs. Each
	/// one is checked against a new observation before anything commits
	/// (ADR-0086).
	pub(crate) fn required_capabilities(&self) -> &'static [Capability] {
		match self {
			Self::SetSetting {
				key: SettingKey::GitAutoCommit,
				value: SettingValue::Flag(true),
				..
			} => GIT,
			Self::BindAccount {
				credential_source: CredentialSource::PlatformStore,
				..
			} => CREDENTIAL_STORE,
			Self::BindAccount { .. }
			| Self::UnbindAccount { .. }
			| Self::BeginAuditEpoch
			| Self::SetPairingGate { .. }
			| Self::OpenPairing { .. }
			| Self::ClaimPairing { .. }
			| Self::CreateConversation { .. }
			| Self::CreateRun { .. }
			| Self::SetSetting { .. }
			| Self::ClearSetting { .. }
			| Self::TransitionRun { .. } => &[],
		}
	}

	/// Whether this Command may run while the Plane cannot vouch for its
	/// Security audit (ADR-0105).
	///
	/// A change the audit exists to record is exactly a change that needs
	/// an audit worth recording it in, so the two answers come from the
	/// same place. Beginning an epoch is the way out and is never guarded.
	pub(crate) fn security_class(&self) -> SecurityClass {
		audit::decision_for(self)
			.map_or(SecurityClass::Ordinary, |_| SecurityClass::Guarded)
	}
}

/// The durable result of a [`Command`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOutcome {
	/// The Conversation as created.
	ConversationCreated(Conversation),
	/// The Run as created.
	RunCreated(Run),
	/// The Run after its transition.
	RunTransitioned(Run),
	/// The Setting value the named scope now stores.
	SettingSet {
		/// The Setting that was stored.
		key: SettingKey,
		/// The scope that stores it.
		scope: SettingScope,
		/// The stored value.
		value: SettingValue,
	},
	/// The named scope no longer stores its own value for the Setting.
	SettingCleared {
		/// The Setting that was cleared.
		key: SettingKey,
		/// The scope that no longer stores a value.
		scope: SettingScope,
	},
	/// The Account binding as established.
	AccountBound(AccountBinding),
	/// Where the Plane's Pairing gate now stands.
	PairingGateSet {
		/// The gate as the Plane now records it.
		gate: PairingGate,
	},
	/// The Pairing offer the Plane now has open, and its one-time secret as
	/// it is disclosed once.
	PairingOpened {
		/// The offer, without the secret it was issued with.
		pending: PendingPairing,
		/// The secret, in the form the owner hands it over in.
		disclosure: PairingDisclosure,
	},
	/// The Pairing offer after a client claimed it, and the fresh challenge
	/// that client's key signs to complete the Pairing.
	PairingClaimed {
		/// The offer, now waiting for the people at both ends.
		pending: PendingPairing,
		/// The challenge to sign.
		challenge: PairingChallenge,
	},
	/// The authority epoch the Security audit now records in.
	AuditEpochBegun {
		/// The epoch that holds the chain the Plane vouches for.
		epoch: AuditEpoch,
	},
	/// The Plane no longer has the binding, and the reference whose secret
	/// its owner may now remove from the backend.
	AccountUnbound {
		/// The binding that was removed.
		binding_id: AccountBindingId,
		/// The reference it resolved through.
		credential_reference: CredentialReference,
	},
}

impl Core {
	/// Executes `command` on behalf of `actor`. The outcome is durable when
	/// this returns `Ok`.
	///
	/// # Errors
	///
	/// Returns a `not_found` [`CoreError`] for unknown identities, a
	/// `conflict` one when the Command violates a lifecycle invariant, and
	/// a store category when the transaction cannot commit.
	pub async fn execute(
		&self,
		actor: &Actor,
		envelope: CommandEnvelope,
	) -> Result<CommandOutcome, CoreError> {
		actor.authorize()?;
		let CommandEnvelope {
			command_id,
			command,
			request_digest,
		} = envelope;
		let actor_record = actor.record();
		let security = *self.security.read().await;
		let recorded_at_unix_ms = self.now_unix_ms();
		if let Err(refusal) = self
			.revalidate_capabilities(actor_record, command_id, &command)
			.await
		{
			// ASVS 16.2.1: a decision the audit would have recorded is
			// recorded when it is refused as well (ADR-0105).
			audit::record_refusal(
				&self.store,
				actor,
				&command,
				recorded_at_unix_ms,
			)
			.await?;
			return Err(refusal);
		}
		let outcome = self
			.store
			.write(async |tx| {
				tx.prune_command_receipts_before(
					recorded_at_unix_ms.saturating_sub(COMMAND_RETENTION_MS),
				)
				.await?;
				if let Some(receipt) =
					tx.command_receipt(actor_record, command_id.0).await?
				{
					return replay(
						receipt,
						request_digest,
						recorded_at_unix_ms,
					);
				}
				// ASVS 16.2.1: a Plane that cannot vouch for its Security
				// audit changes nothing worth recording until an owner has
				// dealt with it (ADR-0105). The check sits behind the
				// receipt above, because a retry of a Command that already
				// committed is not a new mutation (ADR-0093), and it takes
				// the transaction down with it rather than recording an
				// outcome the audit could not be trusted to hold.
				security.admit(command.security_class())?;

				let result = execute_new(
					tx,
					actor,
					command_id,
					command,
					security,
					recorded_at_unix_ms,
				)
				.await;
				if let Err(error) = &result
					&& !error.is_authoritative_result()
				{
					return Err(error.clone());
				}
				// An authoritative error is raised before the Command writes
				// any state, so committing its receipt commits nothing else.
				// A Command that must fail authoritatively after writing
				// wraps its writes in a savepoint first.
				tx.insert_command_receipt(&NewCommandReceipt {
					actor: actor_record,
					command_id: command_id.0,
					request_digest,
					recorded_at_unix_ms,
					outcome_version: OUTCOME_VERSION,
					outcome: encode_result(&for_receipt(&result))?,
				})
				.await?;
				Ok(result)
			})
			.await??;
		// Carrying on past an integrity failure is not the daemon deciding
		// it is well again: it validates the chain it now vouches for.
		if matches!(outcome, CommandOutcome::AuditEpochBegun { .. }) {
			*self.security.write().await =
				SecurityState::of(self.store.validate_audit().await?);
		}
		Ok(outcome)
	}
}

/// What the durable receipt keeps of a Command's result.
///
/// Everything, except a secret the Plane discloses once: the receipt
/// outlives the offer it belongs to by thirty days (ADR-0093), and a
/// pairing code that lived for two minutes has no business being there.
/// The retry is answered with the offer, and its owner opens another one.
fn for_receipt(
	result: &Result<CommandOutcome, CoreError>,
) -> Result<CommandOutcome, CoreError> {
	match result {
		Ok(CommandOutcome::PairingOpened { pending, .. }) => {
			Ok(CommandOutcome::PairingOpened {
				pending: pending.clone(),
				disclosure: PairingDisclosure::AlreadyDisclosed,
			})
		}
		Ok(
			outcome @ (CommandOutcome::ConversationCreated(_)
			| CommandOutcome::RunCreated(_)
			| CommandOutcome::RunTransitioned(_)
			| CommandOutcome::SettingSet { .. }
			| CommandOutcome::SettingCleared { .. }
			| CommandOutcome::AccountBound(_)
			| CommandOutcome::AccountUnbound { .. }
			| CommandOutcome::AuditEpochBegun { .. }
			| CommandOutcome::PairingGateSet { .. }
			| CommandOutcome::PairingClaimed { .. }),
		) => Ok(outcome.clone()),
		Err(error) => Err(error.clone()),
	}
}

async fn execute_new(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	command: Command,
	security: SecurityState,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	match command {
		Command::CreateConversation { retention } => {
			create_conversation(tx, actor, retention, now_unix_ms).await
		}
		Command::CreateRun { conversation_id } => {
			create_run(tx, actor, conversation_id, now_unix_ms).await
		}
		Command::SetSetting { key, scope, value } => {
			set_setting(tx, actor, key, scope, value, now_unix_ms).await
		}
		Command::ClearSetting { key, scope } => {
			clear_setting(tx, actor, key, scope, now_unix_ms).await
		}
		Command::BindAccount {
			provider,
			label,
			provider_account,
			credential_source,
		} => {
			account::bind(
				tx,
				actor,
				account::Requested {
					provider,
					label,
					provider_account,
					credential_source,
				},
				now_unix_ms,
			)
			.await
		}
		Command::UnbindAccount { binding_id } => {
			account::unbind(tx, actor, binding_id, now_unix_ms).await
		}
		Command::BeginAuditEpoch => {
			security::begin_epoch(tx, actor, security, now_unix_ms).await
		}
		Command::SetPairingGate { gate } => {
			pairing::set_gate(tx, actor, gate, now_unix_ms).await
		}
		Command::OpenPairing { method } => {
			pairing_offer::open(tx, actor, method, now_unix_ms).await
		}
		Command::ClaimPairing { secret, key } => {
			pairing_offer::claim(tx, actor, secret, key, now_unix_ms).await
		}
		Command::TransitionRun {
			run_id,
			expected_revision,
			lifecycle,
		} => {
			transition_run(
				tx,
				actor,
				command_id,
				run_id,
				expected_revision,
				lifecycle,
				now_unix_ms,
			)
			.await
		}
	}
}

async fn create_conversation(
	tx: &mut WriteTransaction,
	actor: &Actor,
	retention: RetentionPolicy,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let conversation: Conversation = tx
		.insert_conversation(NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationCreated { retention };
	tx.append_event(event.to_record(
		actor,
		EventSubject::Conversation(conversation.conversation_id),
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::ConversationCreated(conversation))
}

async fn create_run(
	tx: &mut WriteTransaction,
	actor: &Actor,
	conversation_id: ConversationId,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	if tx.conversation(conversation_id.0).await?.is_none() {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	}
	let busy = tx
		.runs(conversation_id.0)
		.await?
		.iter()
		.any(|run| !run.lifecycle.is_terminal());
	if busy {
		return Err(CoreError::conflict(
			"run.conversation_busy",
			"the Conversation already has a Run that has not ended",
		));
	}
	let run: Run = tx
		.insert_run(NewRun {
			run_id: Uuid::now_v7(),
			conversation_id: conversation_id.0,
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	tx.append_event(EventKind::RunCreated {}.to_record(
		actor,
		EventSubject::Run {
			conversation_id,
			run_id: run.run_id,
		},
		now_unix_ms,
	)?)
	.await?;
	Ok(CommandOutcome::RunCreated(run))
}

async fn set_setting(
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

async fn clear_setting(
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

async fn transition_run(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	run_id: RunId,
	expected_revision: Revision,
	lifecycle: RunLifecycle,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
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

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
