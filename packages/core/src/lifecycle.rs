//! The Run lifecycle progression (ADR-0065): `created`, `starting`,
//! `active`, `stopping`, then exactly one terminal result. A Run may jump
//! from any live state to any terminal result, but never skips a live
//! state and never leaves a terminal one.

use jet_store::RunLifecycle;

pub(crate) fn may_transition(from: RunLifecycle, to: RunLifecycle) -> bool {
	!from.is_terminal() && (to.is_terminal() || successor(from) == Some(to))
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

pub(crate) fn name(lifecycle: RunLifecycle) -> &'static str {
	match lifecycle {
		RunLifecycle::Created => "created",
		RunLifecycle::Starting => "starting",
		RunLifecycle::Active => "active",
		RunLifecycle::Stopping => "stopping",
		RunLifecycle::Completed => "completed",
		RunLifecycle::Failed => "failed",
		RunLifecycle::Canceled => "canceled",
		RunLifecycle::Lost => "lost",
	}
}
