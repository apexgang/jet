//! Commands: authenticated, durable mutations. Each one commits its
//! current-state change and its journal Event in one transaction and is
//! acknowledged only after that commit (ADR-0020, ADR-0071).

use jet_store::{
	EffectKindRecord, EffectSafetyRecord, NewCommandReceipt, NewConversation,
	NewEffect, NewRun, PairedClientAccess, PairingGate, PairingMethod,
	RetentionPolicy, RunLifecycle, SettingRecord, WorkingTreeRecord,
	WriteTransaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::account::{
	self, AccountBinding, AccountBindingId, CredentialReference,
	CredentialSource, ProviderAccount, ProviderId,
};
use crate::audit::{self, AuditEpoch, AuditSubject, Decision};
use crate::capability::{Capability, ExternalTool, HarnessId};
use crate::command_receipt::{
	COMMAND_RETENTION_MS, OUTCOME_VERSION, encode_result, replay,
};
use crate::conversation::{
	Conversation, ConversationId, ConversationOrigin, Revision, Run, RunId,
};
use crate::error::{ConflictState, CoreError, RevisionConflict};
use crate::event::{EventKind, EventSubject};
use crate::import::{
	self, ImportId, ImportedConversation, NativeConversationId,
};
use crate::pairing::{
	self, AuthenticationString, ClientPublicKey, PairedClient,
	PairingChallenge, PairingDisclosure, PairingOfferId, PairingSecret,
	PairingSignature, PendingPairing,
};
use crate::preparation::Prepared;
use crate::project::{self, PathGrant, Project};
use crate::promotion::{PromotionBinding, WorkspacePromotion};
use crate::promotion_command;
use crate::security::{self, SecurityClass, SecurityState};
use crate::setting::{self, SettingKey, SettingScope, SettingValue};
use crate::workspace::{self, WorkingTree, WorkingTreeRequest, WorkspaceHome};
use crate::{Actor, ClientId, Core, lifecycle};
use crate::{paired_client, pairing_completion, pairing_offer};

/// Automatic Git delivery is carried out with the Git the core invokes, so
/// turning it on depends on that tool being installed, and a Project is
/// registered only after that Git has looked at it (ADR-0029, ADR-0056,
/// ADR-0103).
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
	/// Start a managed Run with one installed Craft and its initial input.
	StartRun {
		/// The Conversation whose registered working tree is used.
		conversation_id: ConversationId,
		/// Installed Craft identity, never an executable path.
		craft: String,
		/// Initial Harness input.
		prompt: String,
	},
	/// Create a Conversation with no Runs, and the Workspace it works in
	/// when it asks for one (ADR-0025).
	CreateConversation {
		/// Whether Jet keeps the Conversation after its final Run.
		retention: RetentionPolicy,
		/// Where it does its work.
		working_tree: WorkingTreeRequest,
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
	/// Confirm, on the Plane being Paired with, that both screens show the
	/// same authentication string. The client being Paired cannot confirm
	/// its own Pairing (ADR-0017).
	ConfirmPairing {
		/// The offer being confirmed, which must be the one the Plane has
		/// open.
		offer_id: PairingOfferId,
		/// The string as the person confirming reads it.
		authentication_string: AuthenticationString,
	},
	/// Complete the Pairing by signing the transcript of the claim with the
	/// Client identity that made it (ADR-0090).
	CompletePairing {
		/// The offer being completed, which must be the one the Plane has
		/// open.
		offer_id: PairingOfferId,
		/// The signature over the claim's transcript.
		signature: PairingSignature,
	},
	/// Stop a Paired client controlling this Plane, or let it control the
	/// Plane again. The Plane keeps its key either way (ADR-0017).
	SetPairedClientAccess {
		/// The Paired client to decide about.
		client_id: ClientId,
		/// What it may do from now on.
		access: PairedClientAccess,
	},
	/// Forget a Paired client and the key it was Paired with. The
	/// installation has to be Paired again to control this Plane.
	RevokePairedClient {
		/// The client to forget.
		client_id: ClientId,
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
	/// Register the Git working tree an interactive user granted as a
	/// Project (ADR-0025, ADR-0101). The grant is resolved and inspected
	/// before the transaction opens; a directory that is not an ordinary
	/// working tree is refused (ADR-0103).
	RegisterProject {
		/// The user's explicit authorization for one absolute path.
		grant: PathGrant,
	},
	/// Promote a Workspace to the permanent checkout or branch its preview
	/// was made for, exactly as previewed (ADR-0025). The preview is
	/// computed again before the transaction opens, and a Workspace or
	/// destination that moved on makes it stale and refused.
	PromoteWorkspace {
		/// What the preview bound and the user confirmed.
		binding: PromotionBinding,
	},
	/// Register a Harness-native Conversation the Plane can see outside
	/// its management, so a managed Run may later continue it (ADR-0010).
	/// The identity is looked for again before the transaction opens; one
	/// no supported Harness reports is refused.
	ImportConversation {
		/// The Harness whose identity it is.
		harness: HarnessId,
		/// The identity as the Harness spells it.
		native_conversation: NativeConversationId,
	},
	/// Continue an Imported conversation as a new Conversation in a
	/// Workspace or the Local checkout of a registered Project, chosen by
	/// the user (ADR-0010, ADR-0025). Nowhere to work is refused.
	ResumeImportedConversation {
		/// The import to continue.
		import_id: ImportId,
		/// Whether Jet keeps the Conversation after its final Run.
		retention: RetentionPolicy,
		/// Where it does its work.
		working_tree: WorkingTreeRequest,
	},
}

impl Command {
	/// What the Plane must still be able to do when this Command runs. Each
	/// one is checked against a new observation before anything commits
	/// (ADR-0086).
	pub(crate) fn required_capabilities(&self) -> &'static [Capability] {
		match self {
			Self::StartRun { .. } => GIT,
			Self::SetSetting {
				key: SettingKey::GitAutoCommit,
				value: SettingValue::Flag(true),
				..
			}
			| Self::RegisterProject { .. }
			| Self::CreateConversation {
				working_tree: WorkingTreeRequest::Workspace { .. },
				..
			}
			| Self::ResumeImportedConversation {
				working_tree: WorkingTreeRequest::Workspace { .. },
				..
			}
			| Self::PromoteWorkspace { .. } => GIT,
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
			| Self::ConfirmPairing { .. }
			| Self::CompletePairing { .. }
			| Self::SetPairedClientAccess { .. }
			| Self::RevokePairedClient { .. }
			| Self::CreateConversation { .. }
			| Self::CreateRun { .. }
			| Self::ImportConversation { .. }
			| Self::ResumeImportedConversation { .. }
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
	/// The Pairing offer after the person at the target confirmed it.
	PairingConfirmed {
		/// The offer, now waiting for the client to prove its key.
		pending: PendingPairing,
	},
	/// The client this Plane is now Paired with.
	PairingCompleted {
		/// The Paired client the Pairing left behind.
		client: PairedClient,
	},
	/// The Paired client as the Plane now records it.
	PairedClientAccessSet {
		/// The client, with the access it now has.
		client: PairedClient,
	},
	/// The Plane no longer holds that client or its key.
	PairedClientRevoked {
		/// The client that is no longer Paired.
		client_id: ClientId,
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
	/// The Project as registered.
	ProjectRegistered(Project),
	/// The promotion as recorded: applying, with its Effect committed, or
	/// conflicted, with the paths that keep it from being applied.
	WorkspacePromotionRecorded(WorkspacePromotion),
	/// The Imported conversation as registered, with no Conversation
	/// continuing it yet.
	ConversationImported(ImportedConversation),
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
		// ASVS 8.3.2: no admission can slip between revocation's commit and
		// invalidating live authority, including a replayed Command receipt.
		let _access = self
			.remote_access
			.acquire_many(crate::remote::AUTHORITY_READERS)
			.await
			.expect("authority gate never closes");
		actor.authorize(&self.remote_sessions)?;
		let CommandEnvelope {
			command_id,
			command,
			request_digest,
		} = envelope;
		let actor_record = actor.record();
		let security = *self.security.read().await;
		let recorded_at_unix_ms = self.now_unix_ms();
		let prepared = self
			.admit(actor, command_id, &command, recorded_at_unix_ms)
			.await?;
		let mut invalidated_client = None;
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
					prepared,
					TransactionContext {
						security,
						workspace_home: &self.workspace_home,
					},
					recorded_at_unix_ms,
				)
				.await;
				invalidated_client = result
					.as_ref()
					.ok()
					.and_then(crate::remote::invalidated_client);
				if let Err(error) = &result
					&& !error.is_authoritative_result()
				{
					return Err(error.clone());
				}
				// An authoritative error is a durable answer, so its receipt
				// commits together with whatever the Command wrote before
				// raising it. Most write nothing; a refused Pairing claim
				// deliberately writes the attempt it counted, because a
				// Plane that rolled that back would let a client guess for
				// as long as the offer lasts. A Command whose partial
				// writes must not survive its own failure wraps them in a
				// savepoint.
				tx.insert_command_receipt(&NewCommandReceipt {
					actor: actor_record,
					command_id: command_id.0,
					request_digest,
					recorded_at_unix_ms,
					outcome_version: OUTCOME_VERSION,
					outcome: encode_result(&redacted_for_receipt(&result))?,
				})
				.await?;
				Ok(result)
			})
			.await;
		// SQLite may have committed before writing the external audit head
		// failed. Publish the safe direction before propagating either error.
		if let Some(client_id) = invalidated_client {
			self.remote_sessions.invalidate(client_id);
		}
		let outcome = outcome??;
		// Carrying on past an integrity failure is not the daemon deciding
		// it is well again: it validates the chain it now vouches for.
		if matches!(outcome, CommandOutcome::AuditEpochBegun { .. }) {
			*self.security.write().await =
				SecurityState::of(self.store.validate_audit().await?);
		}
		// The Command is durable and acknowledged by its receipt; the
		// index follows in its own transaction, and a start or a later
		// Command finishes what an interruption here leaves (ADR-0036).
		self.index_search().await?;
		Ok(outcome)
	}
}

/// What the durable receipt keeps of a Command's result.
///
/// Everything, except a secret the Plane discloses once: the receipt
/// outlives the offer it belongs to by thirty days (ADR-0093), and a
/// pairing code that lived for two minutes has no business being there.
/// The retry is answered with the offer, and its owner opens another one.
fn redacted_for_receipt(
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
			| CommandOutcome::PairingClaimed { .. }
			| CommandOutcome::PairingConfirmed { .. }
			| CommandOutcome::PairingCompleted { .. }
			| CommandOutcome::PairedClientAccessSet { .. }
			| CommandOutcome::PairedClientRevoked { .. }
			| CommandOutcome::ProjectRegistered(_)
			| CommandOutcome::WorkspacePromotionRecorded(_)
			| CommandOutcome::ConversationImported(_)),
		) => Ok(outcome.clone()),
		Err(error) => Err(error.clone()),
	}
}

/// What the core brings to a Command's transaction that neither the
/// Command nor its preparation carries.
struct TransactionContext<'a> {
	/// Whether the Plane vouched for its Security audit when the Command
	/// was admitted.
	security: SecurityState,
	/// Where the Plane creates Workspaces.
	workspace_home: &'a WorkspaceHome,
}

async fn execute_new(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	command: Command,
	prepared: Prepared,
	context: TransactionContext<'_>,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let TransactionContext {
		security,
		workspace_home,
	} = context;
	match command {
		Command::StartRun {
			conversation_id, ..
		} => {
			let Prepared::Run(plan) = prepared else {
				return Err(CoreError::internal(
					"run.unprepared",
					"missing launch plan",
				));
			};
			crate::run_command::record(
				tx,
				actor,
				command_id,
				conversation_id,
				plan,
				now_unix_ms,
			)
			.await
		}
		Command::PromoteWorkspace { .. } => {
			let Prepared::Promotion(prepared) = prepared else {
				return Err(CoreError::internal(
					"workspace.promotion_unprepared",
					"a Workspace promotion reached its transaction without \
					 its revalidated binding",
				));
			};
			promotion_command::record(
				tx,
				actor,
				command_id,
				prepared,
				now_unix_ms,
			)
			.await
		}
		Command::RegisterProject { .. } => {
			let Prepared::Registration(registrable) = prepared else {
				return Err(CoreError::internal(
					"project.unprepared",
					"a Project registration reached its transaction without \
					 its prepared root",
				));
			};
			project::register(tx, actor, registrable, now_unix_ms).await
		}
		Command::CreateConversation {
			retention,
			working_tree,
		} => match working_tree {
			WorkingTreeRequest::NoProject => {
				create_conversation(tx, actor, retention, now_unix_ms).await
			}
			WorkingTreeRequest::Workspace { .. } => {
				let Prepared::Workspace(prepared) = prepared else {
					return Err(CoreError::internal(
						"workspace.unprepared",
						"a Workspace creation reached its transaction without \
						 its resolved base",
					));
				};
				workspace::create(
					tx,
					actor,
					retention,
					ConversationOrigin::New,
					prepared,
					workspace_home,
					now_unix_ms,
				)
				.await
			}
			WorkingTreeRequest::LocalCheckout { project_id } => {
				workspace::create_in_local_checkout(
					tx,
					actor,
					retention,
					ConversationOrigin::New,
					project_id,
					now_unix_ms,
				)
				.await
			}
		},
		Command::ImportConversation { .. } => {
			import::import(tx, actor, prepared, now_unix_ms).await
		}
		Command::ResumeImportedConversation {
			import_id,
			retention,
			working_tree,
		} => {
			import::resume(
				tx,
				actor,
				import::Resume {
					import_id,
					retention,
					working_tree,
					prepared,
				},
				workspace_home,
				now_unix_ms,
			)
			.await
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
		Command::ConfirmPairing {
			offer_id,
			authentication_string,
		} => {
			pairing_completion::confirm(
				tx,
				actor,
				offer_id,
				authentication_string,
				now_unix_ms,
			)
			.await
		}
		Command::SetPairedClientAccess { client_id, access } => {
			paired_client::set_access(tx, actor, client_id, access, now_unix_ms)
				.await
		}
		Command::RevokePairedClient { client_id } => {
			paired_client::revoke(tx, actor, client_id, now_unix_ms).await
		}
		Command::CompletePairing {
			offer_id,
			signature,
		} => {
			pairing_completion::complete(
				tx,
				actor,
				offer_id,
				signature,
				now_unix_ms,
			)
			.await
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
			working_tree: WorkingTreeRecord::NoProject,
			origin: ConversationOrigin::New.record(),
			created_at_unix_ms: now_unix_ms,
		})
		.await?
		.into();
	let event = EventKind::ConversationCreated {
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
