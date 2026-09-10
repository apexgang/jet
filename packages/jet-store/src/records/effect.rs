//! Transactional outbox records and reconciliation policy.

use uuid::Uuid;

/// Closed durable spelling of external work understood by this release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKindRecord {
	/// One non-destructive Git delivery mutation.
	GitDelivery,
	/// Apply a deferred Harness-native extension operation.
	ChangeExtension,
	/// One bounded Utility inference request.
	Utility,
	/// Start a Workspace terminal.
	StartTerminal,
	/// Close a Workspace terminal.
	CloseTerminal,
	/// Resolve one Orphaned execution after interactive authorization.
	ResolveExecution,
	/// Start one Run's managed processes.
	StartRun,
	/// Carry out one interactive control request against a managed Run
	/// (ADR-0083).
	ControlRun,
	/// Apply one Workspace promotion to its destination (ADR-0025).
	PromoteWorkspace,
	/// Publish one accepted third-party Craft installation (ADR-0013).
	InstallCraft,
}

impl EffectKindRecord {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::ChangeExtension => "extension.change",
			Self::Utility => "utility.infer",
			Self::GitDelivery => "git.delivery",
			Self::StartTerminal => "terminal.start",
			Self::CloseTerminal => "terminal.close",
			Self::ResolveExecution => "execution.resolve",
			Self::StartRun => "run.start",
			Self::ControlRun => "run.control",
			Self::PromoteWorkspace => "workspace.promote",
			Self::InstallCraft => "craft.install",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		match text {
			"extension.change" => Some(Self::ChangeExtension),
			"utility.infer" => Some(Self::Utility),
			"git.delivery" => Some(Self::GitDelivery),
			"terminal.start" => Some(Self::StartTerminal),
			"terminal.close" => Some(Self::CloseTerminal),
			"execution.resolve" => Some(Self::ResolveExecution),
			"run.start" => Some(Self::StartRun),
			"run.control" => Some(Self::ControlRun),
			"workspace.promote" => Some(Self::PromoteWorkspace),
			"craft.install" => Some(Self::InstallCraft),
			_ => None,
		}
	}
}

/// Durable lifecycle of one external-work Effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectStateRecord {
	/// Committed but never handed to its Adapter.
	Pending,
	/// Handed to its Adapter without a durably recorded outcome yet.
	InFlight,
	/// External work completed successfully.
	Completed,
	/// External work returned a definite failure.
	Failed,
	/// Reconciliation could not establish a safe outcome.
	OutcomeUnknown,
}

/// Evidence that determines whether an interrupted Effect may be repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSafetyRecord {
	/// The operation cannot mutate external state.
	ReadOnly {
		/// Maximum number of attempts allowed by policy.
		max_attempts: u32,
	},
	/// The target deduplicates attempts under one stable external key.
	Idempotent {
		/// Key supplied unchanged to the external target on every attempt.
		external_key: Uuid,
		/// Maximum number of attempts allowed by policy.
		max_attempts: u32,
	},
	/// The operation may mutate state and cannot be safely deduplicated.
	Ambiguous,
}

impl EffectSafetyRecord {
	pub(crate) fn columns(self) -> (&'static str, Option<Uuid>, u32) {
		match self {
			Self::ReadOnly { max_attempts } => {
				("read_only", None, max_attempts)
			}
			Self::Idempotent {
				external_key,
				max_attempts,
			} => ("idempotent", Some(external_key), max_attempts),
			Self::Ambiguous => ("ambiguous", None, 1),
		}
	}
}

impl EffectStateRecord {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Pending => "pending",
			Self::InFlight => "in_flight",
			Self::Completed => "completed",
			Self::Failed => "failed",
			Self::OutcomeUnknown => "outcome_unknown",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[
			Self::Pending,
			Self::InFlight,
			Self::Completed,
			Self::Failed,
			Self::OutcomeUnknown,
		]
		.into_iter()
		.find(|state| state.as_str() == text)
	}
}

/// A newly committed request for external work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEffect {
	/// Globally unique identity reused for every attempt.
	pub effect_id: Uuid,
	/// Actor-scoped Command identity that initiated the Effect.
	pub command_id: Uuid,
	/// Run affected by the work.
	pub run_id: Option<Uuid>,
	/// Workspace promotion the work applies.
	pub promotion_id: Option<Uuid>,
	/// Workspace terminal affected by the work.
	pub terminal_id: Option<Uuid>,
	/// Closed Effect kind understood by the core.
	pub kind: EffectKindRecord,
	/// Evidence and retry bound governing interrupted attempts.
	pub safety: EffectSafetyRecord,
}

/// One durable external-work Effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRecord {
	/// Globally unique identity reused for every attempt.
	pub effect_id: Uuid,
	/// Actor-scoped Command identity that initiated the Effect.
	pub command_id: Uuid,
	/// Run affected by the work.
	pub run_id: Option<Uuid>,
	/// Workspace promotion the work applies.
	pub promotion_id: Option<Uuid>,
	/// Workspace terminal affected by the work.
	pub terminal_id: Option<Uuid>,
	/// Closed Effect kind understood by the core.
	pub kind: EffectKindRecord,
	/// Evidence and retry bound governing interrupted attempts.
	pub safety: EffectSafetyRecord,
	/// Durable lifecycle state.
	pub state: EffectStateRecord,
	/// Number of times the Effect was handed to its Adapter.
	pub attempt_count: u32,
}
