//! Explicit translation of queue domain values into versioned wire values.
use jet_core as core;
use jet_protocol as wire;
pub(super) fn source_from_wire(source: wire::TurnSource) -> core::TurnSource {
	match source {
		wire::TurnSource::User => core::TurnSource::User,
		wire::TurnSource::Schedule => core::TurnSource::Schedule,
		wire::TurnSource::AutoContinue => core::TurnSource::AutoContinue,
	}
}
pub(super) fn turn(turn: core::Turn) -> wire::Turn {
	wire::Turn {
		turn_id: turn.turn_id,
		sequence: turn.sequence,
		client_id: turn.client_id.0,
		run_id: turn.run_id.map(|id| id.0),
		source: match turn.source {
			core::TurnSource::User => wire::TurnSource::User,
			core::TurnSource::Schedule => wire::TurnSource::Schedule,
			core::TurnSource::AutoContinue => wire::TurnSource::AutoContinue,
		},
		state: match turn.state {
			core::TurnState::Queued => wire::TurnState::Queued,
			core::TurnState::Active => wire::TurnState::Active,
			core::TurnState::Completed => wire::TurnState::Completed,
			core::TurnState::Superseded => wire::TurnState::Superseded,
			core::TurnState::Canceled => wire::TurnState::Canceled,
			core::TurnState::Withdrawn => wire::TurnState::Withdrawn,
			core::TurnState::Failed => wire::TurnState::Failed,
			core::TurnState::OutcomeUnknown => wire::TurnState::OutcomeUnknown,
		},
	}
}
