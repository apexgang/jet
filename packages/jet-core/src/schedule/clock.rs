//! Civil recurrence and UTC selection. Only unresolved occurrences consult tzdb.
use crate::{CoreError, ScheduleFiring, ScheduledTask};
use chrono::{DateTime, FixedOffset, NaiveDateTime, NaiveTime};
use jiff::{
	Timestamp, civil,
	tz::{AmbiguousOffset, TimeZone},
};
use uuid::Uuid;

fn invalid() -> CoreError {
	CoreError::invalid_input(
		"schedule.invalid_time",
		"use an IANA time zone and a local time in HH:MM:SS form",
	)
}
/// The bundled IANA zone named exactly `name`. The database lookup ignores
/// case and also knows `Factory`, which names no place; a schedule keeps the
/// exact identifier it was created with.
fn time_zone(name: &str) -> Result<TimeZone, CoreError> {
	TimeZone::get(name)
		.ok()
		.filter(|zone| name != "Factory" && zone.iana_name() == Some(name))
		.ok_or_else(invalid)
}
pub(crate) fn first(
	id: Uuid,
	zone: &str,
	time: &str,
	now: i64,
) -> Result<ScheduleFiring, CoreError> {
	let zone = time_zone(zone)?;
	let time_value =
		NaiveTime::parse_from_str(time, "%H:%M:%S").map_err(|_| invalid())?;
	if time_value.format("%H:%M:%S").to_string() != time
		|| time.ends_with(":60")
	{
		return Err(invalid());
	}
	let offset = zone
		.to_offset(Timestamp::from_millisecond(now).map_err(|_| invalid())?);
	let date = DateTime::from_timestamp_millis(now)
		.ok_or_else(invalid)?
		.with_timezone(
			&FixedOffset::east_opt(offset.seconds()).ok_or_else(invalid)?,
		)
		.date_naive();
	let mut local = date.and_time(time_value);
	loop {
		let firing = resolve(id, &zone, local)?;
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
	resolve(task.schedule_id, &time_zone(&task.time_zone)?, local)
}
fn resolve(
	id: Uuid,
	zone: &TimeZone,
	local: NaiveDateTime,
) -> Result<ScheduleFiring, CoreError> {
	let intended_local = local.format("%Y-%m-%dT%H:%M:%S").to_string();
	let civil: civil::DateTime =
		intended_local.parse().map_err(|_| invalid())?;
	let instant = match zone.to_ambiguous_timestamp(civil).offset() {
		AmbiguousOffset::Unambiguous { offset } => {
			offset.to_timestamp(civil).ok()
		}
		// A repeated local time fires once, at its earlier occurrence. jiff
		// 0.2.37 reads local times from the civil start of a zone's last
		// listed transition onward by the zone's POSIX rule, which can report
		// a fold whose earlier offset the zone never had: America/Nuuk was at
		// -02 from 2023-03-26 to 2024-03-31, yet 2023-10-28 23:xx comes back
		// as a fold from -01. The earlier occurrence counts only if the zone
		// is at `before` then.
		AmbiguousOffset::Fold { before, after } => before
			.to_timestamp(civil)
			.ok()
			.filter(|&earlier| zone.to_offset(earlier) == before)
			.or_else(|| after.to_timestamp(civil).ok()),
		// A skipped local time advances to the transition that skipped it.
		AmbiguousOffset::Gap { after, .. } => after
			.to_timestamp(civil)
			.ok()
			.and_then(|ahead| zone.following(ahead).next())
			.map(|transition| transition.timestamp()),
	}
	.ok_or_else(invalid)?;
	Ok(ScheduleFiring {
		firing_id: Uuid::new_v5(&id, intended_local.as_bytes()),
		intended_local,
		due_at_unix_ms: instant.as_millisecond(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use jiff::{SignedDuration, tz::Offset};
	use pretty_assertions::assert_eq;

	#[test]
	fn a_local_time_jiff_misreads_as_a_fold_fires_at_the_offset_in_force() {
		let id = Uuid::nil();
		// America/Nuuk was at -02 from 2023-03-26 to 2024-03-31; jiff 0.2.37
		// reports 2023-10-28 23:00-23:59 as a fold from -01.
		let now = 1_698_494_400_000; // 2023-10-28T12:00:00Z
		assert_eq!(
			first(id, "America/Nuuk", "23:30:00", now).unwrap(),
			ScheduleFiring {
				firing_id: Uuid::new_v5(&id, b"2023-10-28T23:30:00"),
				intended_local: "2023-10-28T23:30:00".into(),
				due_at_unix_ms: 1_698_543_000_000, // 2023-10-29T01:30:00Z
			}
		);
	}

	/// Every bundled zone, around every transition from 1970 to 2100: each
	/// local time resolves to what the zone's instant-to-offset data alone
	/// select, so a jiff or tzdb upgrade cannot shift a firing unnoticed.
	#[test]
	fn every_bundled_zone_resolves_local_times_as_its_offsets_select() {
		let start = Timestamp::from_second(0).unwrap();
		let end: Timestamp = "2100-01-01T00:00:00Z".parse().unwrap();
		let second = SignedDuration::from_secs(1);
		let mut checked = 0;
		let mut wrong = Vec::new();
		for name in jiff::tz::db().available() {
			let zone = TimeZone::get(name.as_str()).unwrap();
			for transition in zone
				.following(start)
				.take_while(|transition| transition.timestamp() < end)
			{
				let at = transition.timestamp();
				for offset in [zone.to_offset(at - second), transition.offset()]
				{
					for delta in [-3600, -1, 0, 1, 1800, 3599, 3600] {
						let local = offset.to_datetime(at)
							+ SignedDuration::from_secs(delta);
						let chosen = resolve(
							Uuid::nil(),
							&zone,
							local.to_string().parse().unwrap(),
						)
						.unwrap()
						.due_at_unix_ms;
						let expected = selected(&zone, local).as_millisecond();
						checked += 1;
						if chosen != expected {
							wrong.push(format!(
								"{} {local}: {chosen} instead of {expected}",
								name.as_str()
							));
						}
					}
				}
			}
		}
		assert!(
			wrong.is_empty(),
			"{} of {checked} local times resolve elsewhere: {:#?}",
			wrong.len(),
			&wrong[..wrong.len().min(10)]
		);
	}

	/// The earliest instant at which `zone` shows `local`, or for a skipped
	/// local time the transition that skipped it, read only from the zone's
	/// instant-to-offset data.
	fn selected(zone: &TimeZone, local: civil::DateTime) -> Timestamp {
		let reach = SignedDuration::from_hours(26);
		let utc = Offset::UTC.to_timestamp(local).unwrap();
		let mut bounds = vec![utc - reach];
		bounds.extend(
			zone.following(utc - reach)
				.map(|transition| transition.timestamp())
				.take_while(|&at| at < utc + reach),
		);
		bounds.push(utc + reach);
		bounds
			.windows(2)
			.find_map(|span| {
				zone.to_offset(span[0])
					.to_timestamp(local)
					.ok()
					.filter(|instant| (span[0]..span[1]).contains(instant))
			})
			.unwrap_or_else(|| {
				bounds[1..bounds.len() - 1]
					.iter()
					.copied()
					.find(|&at| {
						let before = zone
							.to_offset(at - SignedDuration::from_secs(1))
							.to_datetime(at);
						before <= local
							&& local < zone.to_offset(at).to_datetime(at)
					})
					.expect("a local time no instant shows lies in a gap")
			})
	}
}
