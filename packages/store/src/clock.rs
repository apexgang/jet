//! Wall-clock stamp for schema migrations, which run before any core clock
//! exists. Every other row takes its stamp from the caller so one
//! transaction shares one clock. Stamps are display metadata, never an
//! ordering authority (ADR-0069).

pub(crate) fn unix_ms_now() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.ok()
		.and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
		.unwrap_or_default()
}
