//! The Security audit half of the translation seam (ADR-0049, ADR-0105).

use super::unix_ms;
use jet_core::{
	AuditActor, AuditBreach, AuditEntry, AuditHead, AuditOutcome, AuditPage,
	AuditRisk, AuditTarget, SecurityDegradation, SecurityState,
};
use jet_protocol as wire;

pub(super) fn page(
	page: AuditPage,
	minor: u32,
) -> Result<wire::SecurityAudit, jet_core::CoreError> {
	if minor < wire::CRAFT_LIFECYCLE_MINOR
		&& page
			.entries
			.iter()
			.any(|entry| entry.actor == AuditActor::CraftRevocation)
	{
		return Err(jet_core::CoreError {
            category: jet_core::ErrorCategory::Incompatible,
            code: "audit.actor_incompatible".into(),
            retryable: false,
            message: "this audit page includes internal Craft revocations; upgrade the client to read it".into(),
            detail: None,
            revision_conflict: None,
            recovery_actions: vec![],
        });
	}
	Ok(wire::SecurityAudit {
		cursor: page.cursor.0,
		entries: page.entries.into_iter().map(entry).collect(),
	})
}

fn entry(entry: AuditEntry) -> wire::AuditEntry {
	wire::AuditEntry {
		sequence: entry.sequence.0,
		epoch: entry.epoch.0,
		record_id: entry.record_id.0,
		recorded_at_unix_ms: unix_ms(entry.recorded_at),
		plane_id: entry.plane_id.0,
		actor: match entry.actor {
			AuditActor::InteractiveClient { client_id } => {
				wire::AuditActor::InteractiveClient {
					client_id: client_id.0,
				}
			}
			AuditActor::CraftRevocation => wire::AuditActor::CraftRevocation,
		},
		target: target(entry.target),
		decision: entry.decision,
		risk: risk(entry.risk),
		outcome: outcome(entry.outcome),
	}
}

fn target(target: AuditTarget) -> wire::AuditTarget {
	wire::AuditTarget {
		kind: target.kind,
		reference: target.reference.to_string(),
		identity: target.identity,
	}
}

fn risk(risk: AuditRisk) -> wire::AuditRisk {
	match risk {
		AuditRisk::Routine => wire::AuditRisk::Routine,
		AuditRisk::Elevated => wire::AuditRisk::Elevated,
		AuditRisk::Destructive => wire::AuditRisk::Destructive,
	}
}

fn outcome(outcome: AuditOutcome) -> wire::AuditOutcome {
	match outcome {
		AuditOutcome::Succeeded => wire::AuditOutcome::Succeeded,
		AuditOutcome::Denied => wire::AuditOutcome::Denied,
		AuditOutcome::Failed => wire::AuditOutcome::Failed,
	}
}

/// `None` when the audit could not be validated, which only a Plane in
/// read-only Recovery mode reports (ADR-0077).
pub(super) fn security(state: SecurityState) -> Option<wire::SecurityState> {
	match state {
		SecurityState::Trusted => Some(wire::SecurityState::Trusted),
		SecurityState::Degraded(SecurityDegradation {
			breach,
			epoch,
			head,
			store_sequence,
		}) => Some(wire::SecurityState::Degraded {
			breach: audit_breach(breach),
			epoch: epoch.0,
			head: head.map(audit_head),
			store_sequence: store_sequence.0,
		}),
		SecurityState::Unverified => None,
	}
}

fn audit_breach(breach: AuditBreach) -> wire::AuditBreach {
	match breach {
		AuditBreach::HeadMissing => wire::AuditBreach::HeadMissing,
		AuditBreach::HeadNotInStore => wire::AuditBreach::HeadNotInStore,
		AuditBreach::HeadDiverged => wire::AuditBreach::HeadDiverged,
		AuditBreach::RecordAltered { sequence } => {
			wire::AuditBreach::RecordAltered { sequence }
		}
		AuditBreach::TargetAltered { sequence } => {
			wire::AuditBreach::TargetAltered { sequence }
		}
	}
}

fn audit_head(head: AuditHead) -> wire::AuditHead {
	wire::AuditHead {
		epoch: head.epoch,
		sequence: head.sequence,
		entry_hash: head.entry_hash.to_string(),
	}
}
