//! Civil recurrence and UTC selection. Only unresolved occurrences consult tzdb.
use crate::{CoreError, ScheduleFiring, ScheduledTask};
use chrono::{DateTime, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::{GapInfo, Tz};
use uuid::Uuid;
fn invalid() -> CoreError {
	CoreError::invalid_input(
		"schedule.invalid_time",
		"use an IANA time zone and a local time in HH:MM:SS form",
	)
}
pub(crate) fn first(
	id: Uuid,
	zone: &str,
	time: &str,
	now: i64,
) -> Result<ScheduleFiring, CoreError> {
	let zone: Tz = zone.parse().map_err(|_| invalid())?;
	let time_value =
		NaiveTime::parse_from_str(time, "%H:%M:%S").map_err(|_| invalid())?;
	if time_value.format("%H:%M:%S").to_string() != time
		|| time.ends_with(":60")
	{
		return Err(invalid());
	}
	let date = DateTime::from_timestamp_millis(now)
		.ok_or_else(invalid)?
		.with_timezone(&zone)
		.date_naive();
	let mut local = date.and_time(time_value);
	loop {
		let firing = resolve(id, zone, local)?;
		if firing.due_at_unix_ms > now {
			return Ok(firing);
		}
		local = local
			.checked_add_days(chrono::Days::new(1))
			.ok_or_else(invalid)?;
	}
}
pub(crate) fn next(task: &ScheduledTask) -> Result<ScheduleFiring, CoreError> {
	let local = NaiveDateTime::parse_from_str(
		&task.next.intended_local,
		"%Y-%m-%dT%H:%M:%S",
	)
	.map_err(|_| invalid())?
	.checked_add_days(chrono::Days::new(1))
	.ok_or_else(invalid)?;
	resolve(
		task.schedule_id,
		task.time_zone.parse().map_err(|_| invalid())?,
		local,
	)
}
fn resolve(
	id: Uuid,
	zone: Tz,
	local: NaiveDateTime,
) -> Result<ScheduleFiring, CoreError> {
	let instant = zone
		.from_local_datetime(&local)
		.earliest()
		.or_else(|| GapInfo::new(&local, &zone).and_then(|gap| gap.end))
		.ok_or_else(invalid)?;
	let intended_local = local.format("%Y-%m-%dT%H:%M:%S").to_string();
	Ok(ScheduleFiring {
		firing_id: Uuid::new_v5(&id, intended_local.as_bytes()),
		intended_local,
		due_at_unix_ms: instant.timestamp_millis(),
	})
}
