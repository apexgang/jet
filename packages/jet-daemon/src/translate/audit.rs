//! The Security audit half of the translation seam (ADR-0049, ADR-0105).

use jet_core::{
	AuditBreach, AuditEntry, AuditHead, AuditOutcome, AuditPage, AuditRisk,
	AuditTarget, SecurityDegradation, SecurityState,
};
use jet_protocol as wire;

use super::{actor, unix_ms};

pub(super) fn page(page: AuditPage) -> wire::SecurityAudit {
	wire::SecurityAudit {
		cursor: page.cursor.0,
		entries: page.entries.into_iter().map(entry).collect(),
	}
}

fn entry(entry: AuditEntry) -> wire::AuditEntry {
	wire::AuditEntry {
		sequence: entry.sequence.0,
		epoch: entry.epoch.0,
		record_id: entry.record_id.0,
		recorded_at_unix_ms: unix_ms(entry.recorded_at),
		plane_id: entry.plane_id.0,
		actor: actor(&entry.actor),
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

pub(super) fn security(state: SecurityState) -> wire::SecurityState {
	match state {
		SecurityState::Trusted => wire::SecurityState::Trusted,
		SecurityState::Degraded(SecurityDegradation {
			breach,
			epoch,
			head,
			store_sequence,
		}) => wire::SecurityState::Degraded {
			breach: audit_breach(breach),
			epoch: epoch.0,
			head: head.map(audit_head),
			store_sequence: store_sequence.0,
		},
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
