//! Version-gated entity names, and the Event page every feature's
//! minor gate is applied in (ADR-0044, ADR-0019).

use jet_core::{CoreError, EventPage};
use jet_protocol as wire;

pub(super) fn resolved(name: &jet_core::Name) -> wire::Name {
	wire::Name {
		value: name.value.clone(),
		source: match name.source {
			jet_core::NameSource::Manual => wire::NameSource::Manual,
			jet_core::NameSource::Utility => wire::NameSource::Utility,
			jet_core::NameSource::HarnessNative => {
				wire::NameSource::HarnessNative
			}
			jet_core::NameSource::Deterministic => {
				wire::NameSource::Deterministic
			}
		},
	}
}

pub(super) fn event_page(
	page: &EventPage,
	minor: u32,
) -> Result<wire::EventPage, CoreError> {
	Ok(wire::EventPage {
		cursor: page.cursor.0,
		events: page
			.events
			.iter()
			.filter(|event| named(&event.kind, minor))
			.map(|event| super::event(event, minor))
			.collect::<Result<_, _>>()?,
	})
}

/// Whether the negotiated minor names this kind of Event at all. An Event
/// kind is not a field an older reader can skip, so one it never learned
/// is left out of the page rather than sent for it to guess at
/// (ADR-0019).
pub(super) fn named(kind: &jet_core::EventKind, minor: u32) -> bool {
	if matches!(
		kind,
		jet_core::EventKind::ConversationNameChanged { .. }
			| jet_core::EventKind::RunNameChanged { .. }
	) {
		return minor >= wire::NAMES_MINOR;
	}
	if matches!(kind, jet_core::EventKind::UsageRecorded { .. }) {
		return minor >= wire::USAGE_RECORDS_MINOR;
	}
	true
}
