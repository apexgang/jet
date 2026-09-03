//! The Run lifecycle progression (ADR-0065): `created`, `starting`,
//! `active`, `stopping`, then exactly one terminal result. A Run never
//! skips a live state and never leaves a terminal one. It may fail, be
//! canceled, or be lost from any live state, but it only completes after
//! it has been active.

use jet_store::RunLifecycle;

pub(crate) fn may_transition(from: RunLifecycle, to: RunLifecycle) -> bool {
	match to {
		RunLifecycle::Created
		| RunLifecycle::Starting
		| RunLifecycle::Active
		| RunLifecycle::Stopping => successor(from) == Some(to),
		RunLifecycle::Completed => {
			matches!(from, RunLifecycle::Active | RunLifecycle::Stopping)
		}
		RunLifecycle::Failed | RunLifecycle::Canceled | RunLifecycle::Lost => {
			!from.is_terminal()
		}
	}
}

fn successor(lifecycle: RunLifecycle) -> Option<RunLifecycle> {
	match lifecycle {
		RunLifecycle::Created => Some(RunLifecycle::Starting),
		RunLifecycle::Starting => Some(RunLifecycle::Active),
		RunLifecycle::Active => Some(RunLifecycle::Stopping),
		RunLifecycle::Stopping
		| RunLifecycle::Completed
		| RunLifecycle::Failed
		| RunLifecycle::Canceled
		| RunLifecycle::Lost => None,
	}
}
