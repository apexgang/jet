//! Version-gated entity names and name Event pages (ADR-0044).

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
			.filter(|event| {
				minor >= wire::NAMES_MINOR
					|| !matches!(
						&event.kind,
						jet_core::EventKind::ConversationNameChanged { .. }
							| jet_core::EventKind::RunNameChanged { .. }
					)
			})
			.map(|event| super::event(event, minor))
			.collect::<Result<_, _>>()?,
	})
}
