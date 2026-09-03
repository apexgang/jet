//! Wall-clock stamps recorded beside durable rows. They are display
//! metadata, never an ordering authority (ADR-0069).

pub(crate) fn unix_ms_now() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.ok()
		.and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
		.unwrap_or_default()
}
