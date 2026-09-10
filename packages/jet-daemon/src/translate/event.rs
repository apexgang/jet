//! Event payloads, attribution, and timestamps.

use super::{auto_continue, turn};
use jet_core::{ClientId, CoreError, Event, EventPayload};
use jet_protocol as wire;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn event(
	event: &Event,
	minor: u32,
) -> Result<wire::Event, CoreError> {
	let EventPayload {
		kind,
		payload_version,
		mut payload,
	} = event.kind.encode()?;
	if let jet_core::EventKind::AutoContinueChanged { retry } = &event.kind {
		payload =
			serde_json::json!({"retry":auto_continue::retry(*retry.clone())});
	}
	if let jet_core::EventKind::AutoContinueConfigured { target, policy } =
		&event.kind
	{
		payload = serde_json::json!({"target":auto_continue::target_out(*target),"policy":auto_continue::policy_out(policy.clone())});
	}
	if let jet_core::EventKind::TurnChanged { turn: value } = &event.kind {
		payload = serde_json::json!({"turn":turn::turn(value.clone())});
	}
	if minor < wire::NAMES_MINOR
		&& matches!(
			&event.kind,
			jet_core::EventKind::ConversationCreated { .. }
				| jet_core::EventKind::RunCreated { .. }
		) && let Some(payload) = payload.as_object_mut()
	{
		payload.remove("name");
	}
	if minor < wire::NAMES_MINOR
		&& matches!(
			&event.kind,
			jet_core::EventKind::RunProcessesChanged { .. }
		) && let Some(processes) = payload
		.get_mut("processes")
		.and_then(serde_json::Value::as_array_mut)
	{
		for process in processes {
			if let Some(process) = process.as_object_mut() {
				process.remove("label");
			}
		}
	}
	Ok(wire::Event {
		sequence: event.sequence.0,
		event_id: event.event_id.0,
		actor: actor_of(match event.actor {
			jet_core::EventActor::InteractiveClient { client_id } => client_id,
			jet_core::EventActor::Harness { authorized_by, .. }
			| jet_core::EventActor::RunSupervisor { authorized_by, .. }
			| jet_core::EventActor::AutoContinue { authorized_by }
			| jet_core::EventActor::ScheduledTask { authorized_by, .. } => authorized_by,
		}),
		origin: match event.actor {
			jet_core::EventActor::InteractiveClient { .. } => None,
			jet_core::EventActor::AutoContinue { .. } => (minor
				>= wire::AUTO_CONTINUE_MINOR)
				.then_some(wire::EventOrigin::AutoContinue),
			jet_core::EventActor::ScheduledTask { schedule_id, .. } => (minor
				>= wire::SCHEDULES_MINOR)
				.then_some(wire::EventOrigin::ScheduledTask { schedule_id }),
			jet_core::EventActor::Harness { run_id, .. } => {
				Some(wire::EventOrigin::Harness { run_id: run_id.0 })
			}
			jet_core::EventActor::RunSupervisor { run_id, .. } => {
				Some(wire::EventOrigin::RunSupervisor { run_id: run_id.0 })
			}
		},
		recorded_at_unix_ms: unix_ms(event.recorded_at),
		conversation_id: event.conversation_id.map(|id| id.0),
		run_id: event.run_id.map(|id| id.0),
		kind,
		payload_version,
		payload,
	})
}

/// The wire attribution of the Client identity an Actor acted through.
/// Every Actor this core knows is an interactive client, so this is the
/// one place that collapse is spelled.
pub(crate) fn actor_of(client_id: ClientId) -> wire::Actor {
	wire::Actor::InteractiveClient {
		client_id: client_id.0,
	}
}

pub(crate) fn unix_ms(time: SystemTime) -> i64 {
	match time.duration_since(UNIX_EPOCH) {
		Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
		Err(behind) => i64::try_from(behind.duration().as_millis())
			.map_or(i64::MIN, |ms| -ms),
	}
}
