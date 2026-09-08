//! Schedule values cross the wire through explicit DTO conversion.
use jet_core as core;
use jet_protocol as wire;
pub(super) fn task(task: core::ScheduledTask) -> wire::ScheduledTask {
	wire::ScheduledTask {
		schedule_id: task.schedule_id,
		conversation_id: task.conversation_id.0,
		authorized_by: task.authorized_by.0,
		time_zone: task.time_zone,
		local_time: task.local_time,
		prompt: task.prompt,
		next: wire::ScheduleFiring {
			firing_id: task.next.firing_id,
			intended_local: task.next.intended_local,
			due_at_unix_ms: task.next.due_at_unix_ms,
		},
	}
}
