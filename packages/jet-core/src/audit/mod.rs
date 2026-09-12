//! The owner-only Security audit as the core records and reports it
//! (ADR-0105).
//!
//! The audit is not the Event journal. The journal is Conversation history
//! that clients subscribe to and that compaction may thin (ADR-0078); this
//! is a separate, narrower record of the decisions that widen trust, change
//! policy, or destroy state, kept so an owner can answer who did what and
//! when. It is owner-only because reaching a Plane at all is owner-only
//! (ADR-0087).
//!
//! Nothing here can carry a credential, a prompt, terminal output, or file
//! content. A record names a subject and a decision from closed
//! vocabularies this core owns, and the identity it stores is a name for
//! something Jet already keeps — never a value somebody typed.

mod retention;
pub(crate) use retention::sweep_retention;

mod recording;
pub(crate) use recording::{record, record_craft_revocation, record_refusal};

mod policy;
pub(crate) use policy::{
	auto_continue_subject, cleared_setting, decision_for, stored_setting,
};

mod encoding;

pub(crate) mod actor;

use crate::{
	ClientId, PlaneId, ProjectId, account::AccountBindingId,
	conversation::ConversationId, pairing::PairingOfferId,
};
use jet_store::{AuditOutcome, AuditRisk, AuditTargetRef};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

/// Most records one `Query::SecurityAudit` page returns.
pub(crate) const AUDIT_PAGE_LIMIT: usize = jet_store::AUDIT_PAGE_LIMIT;

/// A position in this Plane's Security audit. Positions are never reused,
/// including by the records retention has removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuditSequence(pub u64);

/// One authority epoch of the audit chain. It changes only when an owner
/// explicitly carries on past an integrity failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuditEpoch(pub u64);

/// Durable identity of one audit record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuditRecordId(pub Uuid);

/// A decision worth recording. Each variant is one thing that can be
/// decided, spelled so a person reading the audit a year later still knows
/// what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDecision {
	/// The owner changed an Account-binding default or one-shot retry policy.
	AutoContinuePolicyChanged,
	/// A user reviewed an exact remote action.
	RemoteToolReviewed,
	/// The owner changed Utility routing or purpose permissions.
	UtilityPolicyChanged,
	/// A terminal open was admitted.
	TerminalOpened,
	/// A terminal close was admitted.
	TerminalClosed,
	/// A client submitted ephemeral terminal input; bytes are excluded.
	TerminalInput,
	/// An interactive user requested resolution of an Orphaned execution.
	ExecutionResolutionRequested,
	/// A remote connection attempted to prove its Paired Client identity.
	ConnectionAuthenticated,
	/// A Provider account was bound to this Plane, widening what may
	/// authenticate on it (ADR-0016).
	AccountBound,
	/// An Account binding was removed.
	AccountUnbound,
	/// Automatic Git delivery was turned on for a scope, letting Jet commit
	/// Harness changes there without being asked (ADR-0029).
	GitAutomationEnabled,
	/// It was turned off for that scope.
	GitAutomationDisabled,
	/// A scope stopped pinning it, so the scope above decides again.
	GitAutomationCleared,
	/// The Plane changed how long it keeps this audit (ADR-0105).
	AuditRetentionChanged,
	/// The Plane went back to keeping it for the built-in window.
	AuditRetentionCleared,
	/// An owner carried on past an integrity failure, beginning an
	/// authority epoch that records the gap it leaves behind.
	AuditEpochBegun,
	/// An owner restored a verified Recovery snapshot over a damaged
	/// store, moving authoritative state back to when it was taken
	/// (ADR-0077).
	RecoverySnapshotRestored,
	/// The Plane began accepting new Pairings, so a GUI client that holds a
	/// current pairing code may take control of it (ADR-0017).
	PairingGateOpened,
	/// It stopped accepting them. The clients already Paired are unaffected.
	PairingGateClosed,
	/// The Plane issued a Pairing offer, so a client that presents its
	/// secret in the next two minutes can take control of it (ADR-0017).
	PairingOffered,
	/// A client presented that secret and its durable public key.
	PairingClaimed,
	/// An offer was killed after too many wrong secrets, which is what an
	/// attempt to guess one looks like.
	PairingOfferInvalidated,
	/// The person at the target agreed that both screens showed the same
	/// authentication string.
	PairingConfirmed,
	/// A Pairing completed, so a GUI client now controls this Plane with
	/// full trust (ADR-0017).
	PairingCompleted,
	/// A Paired client was allowed to control this Plane again.
	PairedClientEnabled,
	/// A Paired client was stopped from controlling it. The Plane keeps its
	/// key, so this is not the end of the pairing.
	PairedClientDisabled,
	/// A Paired client and its key were forgotten. Nothing in Jet brings
	/// either back: the installation pairs again or it does not control
	/// this Plane.
	PairedClientRevoked,
	/// An interactive user granted Jet a directory as a Project, widening
	/// what it may read and change on this Plane (ADR-0101).
	ProjectRegistered,
	/// An owner accepted one third-party Craft's exact executable authority.
	CraftInstallationApproved,
	/// Native extension lifecycle request or outcome.
	ExtensionInstall,
	/// Native extension update.
	ExtensionUpdate,
	/// Native extension disable.
	ExtensionDisable,
	/// Native extension removal.
	ExtensionRemove,
	/// An interactive user disabled a Craft.
	CraftDisabled,
	/// The Plane began accepting unverified local or source-built Crafts.
	DeveloperModeEnabled,
	/// The Plane stopped accepting new unverified Craft installations.
	DeveloperModeDisabled,
	/// The Plane returned Developer Mode to its disabled built-in default.
	DeveloperModeCleared,
	/// Automatic review answered exactly one Harness approval request that
	/// would otherwise have waited for a person (ADR-0012).
	ApprovalReviewed,
	/// An interactive user authorized a single exact-action review retry.
	ApprovalRetryAuthorized,
	/// The owner changed whether, or through which binding, this Plane
	/// reviews approval requests automatically.
	ReviewPolicyChanged,
}

/// What a decision is about. The core turns each one into the durable kind
/// and identity the store keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuditSubject {
	/// One staged native extension mutation.
	Extension(uuid::Uuid),
	/// One installed or proposed third-party Craft.
	Craft(String),
	Terminal(crate::TerminalId),
	/// One helper execution, including unmatched identities.
	Execution(crate::RunId),
	/// The Plane as a whole.
	Plane,
	/// One registered Project.
	Project(ProjectId),
	/// One Conversation.
	Conversation(ConversationId),
	/// One Account binding.
	AccountBinding(AccountBindingId),
	/// One Pairing offer.
	PairingOffer(PairingOfferId),
	/// One Paired client.
	PairedClient(ClientId),
}

/// One decision to record beside the change that carried it out.
pub(crate) struct Decision {
	/// What was decided.
	pub(crate) decision: AuditDecision,
	/// What it was about.
	pub(crate) subject: AuditSubject,
	/// What became of it.
	pub(crate) outcome: AuditOutcome,
}

impl Decision {
	/// A decision that was carried out. It is the only outcome a Command
	/// records from inside its own transaction, because a Command that
	/// failed never reaches the commit that would have kept the record.
	pub(crate) fn succeeded(
		decision: AuditDecision,
		subject: AuditSubject,
	) -> Self {
		Self {
			decision,
			subject,
			outcome: AuditOutcome::Succeeded,
		}
	}
}

/// What one recorded decision was about.
///
/// The kind is the durable spelling rather than a closed enum, and the
/// identity is text rather than a parsed one: a core reads an audit that a
/// newer core may have written, and a record it cannot name is still a
/// record it has to show (ADR-0073, ADR-0094).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditTarget {
	/// The durable kind spelling, such as `account_binding`.
	pub kind: String,
	/// The opaque identifier the integrity chain covers. It is derived from
	/// the target, so it outlives the target itself and groups the records
	/// about one thing together after that thing is gone.
	pub reference: AuditTargetRef,
	/// The target's own identity, while the Plane still keeps the target.
	/// Deleting the target clears it and leaves the reference (ADR-0105).
	pub identity: Option<String>,
}

/// One recorded decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
	/// Where it sits in this Plane's audit.
	pub sequence: AuditSequence,
	/// The authority epoch it belongs to.
	pub epoch: AuditEpoch,
	/// Durable identity.
	pub record_id: AuditRecordId,
	/// When the decision was made.
	pub recorded_at: SystemTime,
	/// The Plane that made it.
	pub plane_id: PlaneId,
	/// The responsible origin, which grants no Command authority.
	pub actor: crate::AuditActor,
	/// What it was about.
	pub target: AuditTarget,
	/// The durable spelling of what was decided, such as `account.bound`.
	pub decision: String,
	/// How much it could cost, as judged when it was made.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
}

/// One page of the Security audit, fenced by the position the audit had
/// reached when the page was read. The page is the last one when its final
/// record's sequence equals `cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPage {
	/// Newest audit position when the page was read.
	pub cursor: AuditSequence,
	/// The records strictly after the requested position, oldest first.
	pub entries: Vec<AuditEntry>,
}

#[cfg(test)]
pub(crate) mod tests {
	use std::sync::Arc;
	use std::time::{Duration, SystemTime, UNIX_EPOCH};

	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::audit::{self, AuditSubject};
	use crate::clock::Clock;
	use crate::test_support::{
		FixedProbe, ManualClock, actor, equipped, register_repository, request,
		start_core_with, stripped,
	};
	use crate::{
		AuditDecision, AuditEntry, AuditEpoch, AuditOutcome, AuditPage,
		AuditRisk, AuditSequence, AuditTarget, ClientId, Command,
		CommandOutcome, Core, CredentialSource, ErrorCategory, PlaneId, Query,
		QueryResult, SettingKey, SettingScope, SettingValue,
	};

	/// The built-in retention window, so a test can step past it.
	const RETAINED_FOR: Duration = Duration::from_secs(365 * 24 * 60 * 60);

	/// A fixed instant, so a recorded decision has an exact time rather than
	/// whatever the machine's clock said.
	const NOW: Duration = Duration::from_millis(1_700_000_000_000);

	async fn start(dir: &tempfile::TempDir) -> Core {
		start_core_with(
			&dir.path().join("plane.sqlite3"),
			ManualClock::at(UNIX_EPOCH + NOW),
			FixedProbe::new(equipped()),
		)
		.await
	}

	async fn audit(core: &Core, after: AuditSequence) -> AuditPage {
		let result = core
			.query(&actor(), Query::SecurityAudit { after })
			.await
			.unwrap();
		let QueryResult::SecurityAudit(page) = result else {
			panic!("expected QueryResult::SecurityAudit");
		};
		page
	}

	/// What each recorded decision says, without the identities a test cannot
	/// predict.
	fn decisions(page: &AuditPage) -> Vec<(&str, AuditRisk, AuditOutcome)> {
		page.entries
			.iter()
			.map(|entry| (entry.decision.as_str(), entry.risk, entry.outcome))
			.collect()
	}

	async fn bind(core: &Core, label: &str) -> Uuid {
		let outcome = core
			.execute(
				&actor(),
				request(Command::BindAccount {
					provider: crate::ProviderId("anthropic".into()),
					label: label.into(),
					provider_account: None,
					credential_source: CredentialSource::PlatformStore,
				}),
			)
			.await
			.unwrap();
		let CommandOutcome::AccountBound(binding) = outcome else {
			panic!("expected CommandOutcome::AccountBound");
		};
		binding.binding_id.0
	}

	#[tokio::test]
	async fn binding_an_account_is_recorded_as_an_elevated_decision() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let plane_id = match core.query(&actor(), Query::Status).await.unwrap()
		{
			QueryResult::Status(status) => status.plane_id,
			_ => panic!("expected QueryResult::Status"),
		};

		let binding_id = bind(&core, "Work account").await;

		let page = audit(&core, AuditSequence(0)).await;
		let [entry] = page.entries.as_slice() else {
			panic!("expected one audit entry");
		};
		assert_eq!(
			page,
			AuditPage {
				cursor: AuditSequence(1),
				entries: vec![AuditEntry {
					sequence: AuditSequence(1),
					epoch: AuditEpoch(1),
					record_id: entry.record_id,
					recorded_at: UNIX_EPOCH + NOW,
					plane_id: PlaneId(plane_id.0),
					actor: crate::AuditActor::InteractiveClient {
						client_id: actor().client_id(),
					},
					target: AuditTarget {
						kind: "account_binding".into(),
						reference: entry.target.reference,
						identity: Some(binding_id.to_string()),
					},
					decision: AuditDecision::AccountBound.as_str().into(),
					risk: AuditRisk::Elevated,
					outcome: AuditOutcome::Succeeded,
				}],
			}
		);
	}

	#[tokio::test]
	async fn decisions_about_one_binding_share_its_opaque_reference() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;

		let kept = bind(&core, "Kept").await;
		let removed = bind(&core, "Removed").await;
		core.execute(
			&actor(),
			request(Command::UnbindAccount {
				binding_id: crate::AccountBindingId(removed),
			}),
		)
		.await
		.unwrap();

		let page = audit(&core, AuditSequence(0)).await;
		let [kept_bound, removed_bound, removed_unbound] =
			page.entries.as_slice()
		else {
			panic!("expected three audit entries");
		};
		assert_eq!(
			(
				decisions(&page),
				removed_bound.target.reference
					== removed_unbound.target.reference,
				kept_bound.target.reference == removed_bound.target.reference,
				kept.to_string() == kept_bound.target.identity.clone().unwrap(),
			),
			(
				vec![
					(
						"account.bound",
						AuditRisk::Elevated,
						AuditOutcome::Succeeded
					),
					(
						"account.bound",
						AuditRisk::Elevated,
						AuditOutcome::Succeeded
					),
					(
						"account.unbound",
						AuditRisk::Elevated,
						AuditOutcome::Succeeded
					),
				],
				true,
				false,
				true,
			)
		);
	}

	#[tokio::test]
	async fn git_automation_is_a_policy_decision_and_naming_is_a_preference() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let project = SettingScope::Project {
			project_id: register_repository(&core, &dir.path().join("repo"))
				.await,
		};

		for command in [
			Command::SetSetting {
				key: SettingKey::GitAutoCommit,
				scope: project,
				value: SettingValue::Flag(true),
			},
			Command::SetSetting {
				key: SettingKey::UtilityAutomaticNaming,
				scope: project,
				value: SettingValue::Flag(false),
			},
			Command::SetSetting {
				key: SettingKey::GitAutoCommit,
				scope: project,
				value: SettingValue::Flag(false),
			},
			Command::ClearSetting {
				key: SettingKey::GitAutoCommit,
				scope: project,
			},
		] {
			core.execute(&actor(), request(command)).await.unwrap();
		}

		let page = audit(&core, AuditSequence(0)).await;
		assert_eq!(
			(decisions(&page), page.entries[1].target.kind.as_str()),
			(
				vec![
					(
						"project.registered",
						AuditRisk::Elevated,
						AuditOutcome::Succeeded
					),
					(
						"policy.git_automation_enabled",
						AuditRisk::Elevated,
						AuditOutcome::Succeeded
					),
					(
						"policy.git_automation_disabled",
						AuditRisk::Routine,
						AuditOutcome::Succeeded
					),
					(
						"policy.git_automation_cleared",
						AuditRisk::Elevated,
						AuditOutcome::Succeeded
					),
				],
				"project"
			)
		);
	}

	#[tokio::test]
	async fn the_audit_keeps_no_text_a_client_supplied() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let secret_looking_label = "sk-live-not-actually-a-secret";

		bind(&core, secret_looking_label).await;

		let page = audit(&core, AuditSequence(0)).await;
		let recorded = format!("{page:?}");
		assert!(!recorded.contains(secret_looking_label));
	}

	#[tokio::test]
	async fn an_audit_page_resumes_after_the_position_it_is_given() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;

		bind(&core, "First").await;
		bind(&core, "Second").await;

		let page = audit(&core, AuditSequence(1)).await;
		assert_eq!(
			(page.cursor, page.entries.len(), page.entries[0].sequence),
			(AuditSequence(2), 1, AuditSequence(2))
		);
	}

	#[tokio::test]
	async fn an_actor_is_recorded_with_the_decision_it_made() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let client = crate::Actor::InteractiveClient {
			client_id: ClientId(Uuid::nil()),
		};

		bind(&core, "Work account").await;

		let page = audit(&core, AuditSequence(0)).await;
		assert_eq!(
			page.entries[0].actor,
			crate::AuditActor::InteractiveClient {
				client_id: client.client_id(),
			}
		);
	}

	/// The clock a Command reads once is the clock the audit records, so a
	/// decision and the Event beside it never disagree about when they
	/// happened.
	#[tokio::test]
	async fn a_decision_is_recorded_at_the_time_its_command_read() {
		let dir = tempfile::tempdir().unwrap();
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;

		bind(&core, "First").await;
		clock.advance(Duration::from_secs(60));
		bind(&core, "Second").await;

		let page = audit(&core, AuditSequence(0)).await;
		assert_eq!(
			page.entries
				.iter()
				.map(|entry| entry.recorded_at)
				.collect::<Vec<SystemTime>>(),
			vec![UNIX_EPOCH + NOW, UNIX_EPOCH + NOW + Duration::from_secs(60)]
		);
	}

	#[tokio::test]
	async fn the_retention_window_is_a_destructive_decision_with_a_floor() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;

		let accepted = core
			.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::SecurityAuditRetentionDays,
					scope: SettingScope::Plane,
					value: SettingValue::Count(120),
				}),
			)
			.await;
		let refused = core
			.execute(
				&actor(),
				request(Command::SetSetting {
					key: SettingKey::SecurityAuditRetentionDays,
					scope: SettingScope::Plane,
					value: SettingValue::Count(30),
				}),
			)
			.await
			.unwrap_err();

		assert_eq!(
			(
				accepted.is_ok(),
				(refused.category, refused.code.as_str()),
				decisions(&audit(&core, AuditSequence(0)).await)
			),
			(
				true,
				(ErrorCategory::InvalidInput, "setting.value_below_minimum"),
				vec![(
					"policy.audit_retention_changed",
					AuditRisk::Destructive,
					AuditOutcome::Succeeded
				)]
			)
		);
	}

	#[tokio::test]
	async fn expired_decisions_are_gone_when_the_daemon_starts_again() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn Clock>,
			FixedProbe::new(equipped()),
		)
		.await;

		bind(&core, "First").await;
		bind(&core, "Second").await;
		let kept = bind(&core, "Third").await;
		drop(core);

		clock.advance(RETAINED_FOR + Duration::from_secs(1));
		let restarted = start_core_with(
			&path,
			Arc::clone(&clock) as Arc<dyn Clock>,
			FixedProbe::new(equipped()),
		)
		.await;

		let page = audit(&restarted, AuditSequence(0)).await;
		assert_eq!(
			page.entries
				.iter()
				.map(|entry| (
					entry.sequence,
					entry.target.identity.clone().unwrap()
				))
				.collect::<Vec<_>>(),
			vec![(AuditSequence(3), kept.to_string())]
		);
	}

	#[tokio::test]
	async fn a_deleted_target_keeps_its_reference_and_loses_its_name() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let binding_id = bind(&core, "Work account").await;
		let before = audit(&core, AuditSequence(0)).await;

		let anonymized = core
			.store
			.write(async |tx| {
				audit::retention::anonymize(
					tx,
					AuditSubject::AccountBinding(crate::AccountBindingId(
						binding_id,
					)),
				)
				.await
			})
			.await
			.unwrap();

		let after = audit(&core, AuditSequence(0)).await;
		assert_eq!(
			(anonymized, after),
			(
				1,
				AuditPage {
					cursor: before.cursor,
					entries: vec![AuditEntry {
						target: AuditTarget {
							identity: None,
							..before.entries[0].target.clone()
						},
						..before.entries[0].clone()
					}],
				}
			)
		);
	}

	/// The retention window and the audit that records changing it must agree
	/// about which Plane they belong to, so the value a Plane resolves is the
	/// one its own scope stores.
	#[tokio::test]
	async fn clearing_the_retention_window_returns_to_the_built_in_one() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;

		for command in [
			Command::SetSetting {
				key: SettingKey::SecurityAuditRetentionDays,
				scope: SettingScope::Plane,
				value: SettingValue::Count(120),
			},
			Command::ClearSetting {
				key: SettingKey::SecurityAuditRetentionDays,
				scope: SettingScope::Plane,
			},
		] {
			core.execute(&actor(), request(command)).await.unwrap();
		}

		let resolved = match core
			.query(
				&actor(),
				Query::Settings {
					scope: SettingScope::Plane,
					selection: crate::SettingSelection::Key(
						SettingKey::SecurityAuditRetentionDays,
					),
				},
			)
			.await
			.unwrap()
		{
			QueryResult::Settings(snapshot) => snapshot.settings,
			_ => panic!("expected QueryResult::Settings"),
		};

		assert_eq!(
			(
				resolved
					.into_iter()
					.map(|setting| setting.value)
					.collect::<Vec<_>>(),
				decisions(&audit(&core, AuditSequence(0)).await)
			),
			(
				vec![SettingValue::Count(365)],
				vec![
					(
						"policy.audit_retention_changed",
						AuditRisk::Destructive,
						AuditOutcome::Succeeded
					),
					(
						"policy.audit_retention_cleared",
						AuditRisk::Destructive,
						AuditOutcome::Succeeded
					),
				]
			)
		);
	}

	/// ADR-0105 asks for the failures as much as the successes. A Plane with no
	/// credential store cannot bind an account through one, and refusing to is
	/// how an authentication setup fails here.
	#[tokio::test]
	async fn a_binding_refused_by_the_plane_is_recorded_as_denied() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(stripped());
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			ManualClock::at(UNIX_EPOCH + NOW),
			Arc::clone(&probe),
		)
		.await;

		let refused = core
			.execute(
				&actor(),
				request(Command::BindAccount {
					provider: crate::ProviderId("anthropic".into()),
					label: "Work account".into(),
					provider_account: None,
					credential_source: CredentialSource::PlatformStore,
				}),
			)
			.await
			.unwrap_err();

		let page = audit(&core, AuditSequence(0)).await;
		assert_eq!(
			(
				refused.code.as_str(),
				decisions(&page),
				page.entries[0].target.kind.as_str(),
			),
			(
				"capability.unavailable",
				vec![(
					"account.bound",
					AuditRisk::Elevated,
					AuditOutcome::Denied
				)],
				"plane",
			)
		);
	}

	#[tokio::test]
	async fn auto_continue_policy_audit_excludes_the_message() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let binding = crate::test_support::bind_native_account(
			&core,
			crate::ProviderId("anthropic".into()),
		)
		.await;
		core.execute(
			&actor(),
			request(Command::SetAutoContinue {
				target: crate::AutoContinueTarget::AccountBinding(binding),
				policy: crate::AutoContinuePolicy::Retry {
					delay_ms: 1000,
					max_delay_ms: 2000,
					max_retries: 2,
					message: "private continuation".into(),
				},
			}),
		)
		.await
		.unwrap();
		let page = audit(&core, AuditSequence(0)).await;
		let record = page.entries.last().unwrap();
		assert_eq!(
			(
				record.decision.as_str(),
				record.target.kind.as_str(),
				record.target.identity.as_deref()
			),
			(
				"policy.auto_continue_changed",
				"account_binding",
				Some(binding.0.to_string().as_str())
			)
		);
		assert!(!format!("{page:?}").contains("private continuation"));
	}
}
