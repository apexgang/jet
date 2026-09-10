//! Auto-continue DTO conversion preserves the selected policy and Usage condition.
use jet_core as core;
use jet_protocol as wire;

pub(super) fn target(
	value: wire::AutoContinueTarget,
) -> core::AutoContinueTarget {
	match value {
		wire::AutoContinueTarget::AccountBinding(id) => {
			core::AutoContinueTarget::AccountBinding(core::AccountBindingId(id))
		}
		wire::AutoContinueTarget::Conversation(id) => {
			core::AutoContinueTarget::Conversation(core::ConversationId(id))
		}
	}
}
pub(super) fn target_out(
	value: core::AutoContinueTarget,
) -> wire::AutoContinueTarget {
	match value {
		core::AutoContinueTarget::AccountBinding(id) => {
			wire::AutoContinueTarget::AccountBinding(id.0)
		}
		core::AutoContinueTarget::Conversation(id) => {
			wire::AutoContinueTarget::Conversation(id.0)
		}
	}
}
pub(super) fn policy(
	value: wire::AutoContinuePolicy,
) -> core::AutoContinuePolicy {
	match value {
		wire::AutoContinuePolicy::Off => core::AutoContinuePolicy::Off,
		wire::AutoContinuePolicy::Retry {
			delay_ms,
			max_delay_ms,
			max_retries,
			message,
		} => core::AutoContinuePolicy::Retry {
			delay_ms,
			max_delay_ms,
			max_retries,
			message,
		},
	}
}
pub(super) fn policy_out(
	value: core::AutoContinuePolicy,
) -> wire::AutoContinuePolicy {
	match value {
		core::AutoContinuePolicy::Off => wire::AutoContinuePolicy::Off,
		core::AutoContinuePolicy::Retry {
			delay_ms,
			max_delay_ms,
			max_retries,
			message,
		} => wire::AutoContinuePolicy::Retry {
			delay_ms,
			max_delay_ms,
			max_retries,
			message,
		},
	}
}
pub(super) fn snapshot(
	value: core::AutoContinueSnapshot,
) -> wire::AutoContinueSnapshot {
	wire::AutoContinueSnapshot {
		cursor: value.cursor.0,
		policy: policy_out(value.policy),
		retry: value.retry.map(retry),
	}
}
pub(super) fn retry(r: core::AutoContinueRetry) -> wire::AutoContinueRetry {
	wire::AutoContinueRetry {
		run_id: r.run_id.0,
		triggering_turn: r.triggering_turn,
		retry_turn: r.retry_turn,
		observed_at_unix_ms: r.observed_at_unix_ms,
		due_at_unix_ms: r.due_at_unix_ms,
		retry_count: r.retry_count,
		policy: policy_out(r.policy),
		selected_from: target_out(r.selected_from),
		status: match r.status {
			core::AutoContinueStatus::Disabled => {
				wire::AutoContinueStatus::Disabled
			}
			core::AutoContinueStatus::Deferred => {
				wire::AutoContinueStatus::Deferred
			}
			core::AutoContinueStatus::Pending => {
				wire::AutoContinueStatus::Pending
			}
			core::AutoContinueStatus::Dispatched => {
				wire::AutoContinueStatus::Dispatched
			}
			core::AutoContinueStatus::Canceled => {
				wire::AutoContinueStatus::Canceled
			}
			core::AutoContinueStatus::Exhausted => {
				wire::AutoContinueStatus::Exhausted
			}
		},
		usage: wire::AutoContinueUsage {
			window: r.usage.window,
			scope: super::usage::scope(r.usage.scope),
			measure: super::usage::measure(r.usage.measure),
			window_seconds: r.usage.window_seconds,
			resets_in_seconds: r.usage.resets_in_seconds,
			estimation: super::usage::estimation(r.usage.estimation),
			finality: super::usage::finality(r.usage.finality),
		},
	}
}
