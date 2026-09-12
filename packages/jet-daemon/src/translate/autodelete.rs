//! Autodelete rules across the wire (ADR-0015). Everything here needs
//! protocol minor 40.

use super::unix_ms;
use jet_core::{
	AutodeleteCandidate, AutodeleteRule, AutodeleteRulePreview,
	AutodeleteRuleState, AutodeleteRules, AutodeleteScope,
};
use jet_protocol as wire;

pub(super) fn rule(rule: AutodeleteRule) -> wire::AutodeleteRule {
	wire::AutodeleteRule {
		rule_id: rule.rule_id.0,
		prompt: rule.prompt,
		utility_job_id: rule.utility_job_id,
		state: match rule.state {
			AutodeleteRuleState::Compiling => {
				wire::AutodeleteRuleState::Compiling
			}
			AutodeleteRuleState::Refused { reason } => {
				wire::AutodeleteRuleState::Refused { reason }
			}
			AutodeleteRuleState::Draft { inactive_days } => {
				wire::AutodeleteRuleState::Draft { inactive_days }
			}
			AutodeleteRuleState::Approved {
				inactive_days,
				approved_at,
			} => wire::AutodeleteRuleState::Approved {
				inactive_days,
				approved_at_unix_ms: unix_ms(approved_at),
			},
		},
		scope: match rule.scope {
			AutodeleteScope::Forget => wire::AutodeleteScope::Forget,
			AutodeleteScope::Everywhere => wire::AutodeleteScope::Everywhere,
		},
		created_at_unix_ms: unix_ms(rule.created_at),
		updated_at_unix_ms: unix_ms(rule.updated_at),
	}
}

fn candidate(candidate: AutodeleteCandidate) -> wire::AutodeleteCandidate {
	wire::AutodeleteCandidate {
		conversation_id: candidate.conversation_id.0,
		last_active_at_unix_ms: unix_ms(candidate.last_active_at),
		protections: candidate
			.protections
			.into_iter()
			.map(super::retention::protection)
			.collect(),
	}
}

fn preview(preview: AutodeleteRulePreview) -> wire::AutodeleteRulePreview {
	wire::AutodeleteRulePreview {
		rule: rule(preview.rule),
		candidates: preview.candidates.into_iter().map(candidate).collect(),
	}
}

pub(super) fn rules(rules: AutodeleteRules) -> wire::AutodeleteRules {
	wire::AutodeleteRules {
		cursor: rules.cursor.0,
		rules: rules.rules.into_iter().map(preview).collect(),
	}
}
