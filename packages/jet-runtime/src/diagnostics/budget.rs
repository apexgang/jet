//! How many records the Diagnostic log admits, and how the rest is
//! accounted for rather than lost silently.
//!
//! Two shapes of overload matter. A burst, where an Event storm or a
//! flapping connection produces many different records at once, is held
//! to a token bucket: what does not fit is counted, and the count is
//! written once the bucket admits again. A repeat, where the same failure
//! recurs on every retry, is collapsed: the first is written, the rest are
//! counted, and the count is written when something else happens. Either
//! way the log grows in proportion to what changed, not to how often it
//! was tried (ADR-0061).

use super::{DiagnosticComponent, DiagnosticLevel};
use std::time::{Duration, Instant};

/// How many records a quiet log may admit at once.
const BURST: u32 = 200;

/// How many records a busy log admits per second, sustained.
const SUSTAINED_PER_SECOND: u32 = 20;

/// How long an identical record keeps collapsing into the one before it.
const REPEAT_WINDOW: Duration = Duration::from_secs(60);

/// What tells two records apart for collapsing purposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepeatKey {
	pub(super) level: DiagnosticLevel,
	pub(super) component: DiagnosticComponent,
	pub(super) message: &'static str,
	pub(super) code: Option<String>,
}

/// What the budget decided for one record.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Admission {
	/// Write it, after these accounts of what was not written before it.
	Admitted(Vec<Elision>),
	/// Do not write it; it is counted.
	Suppressed,
}

/// An account of records that were counted rather than written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Elision {
	/// The record before this one recurred this many more times.
	Repeated(u64),
	/// This many different records did not fit the burst budget.
	Suppressed(u64),
}

#[derive(Debug)]
pub(super) struct Budget {
	tokens: u32,
	refilled_at: Instant,
	suppressed: u64,
	last: Option<(RepeatKey, Instant)>,
	repeats: u64,
}

impl Budget {
	pub(super) fn new(now: Instant) -> Self {
		Self {
			tokens: BURST,
			refilled_at: now,
			suppressed: 0,
			last: None,
			repeats: 0,
		}
	}

	pub(super) fn admit(&mut self, key: RepeatKey, now: Instant) -> Admission {
		self.refill(now);
		if let Some((last, at)) = &self.last
			&& *last == key
			&& now.saturating_duration_since(*at) < REPEAT_WINDOW
		{
			// The window is anchored at the last written record, so a
			// failure that never stops is still written once a minute.
			self.repeats += 1;
			return Admission::Suppressed;
		}
		if self.tokens == 0 {
			self.suppressed += 1;
			return Admission::Suppressed;
		}
		self.tokens -= 1;
		let mut elisions = Vec::new();
		if self.repeats > 0 {
			elisions.push(Elision::Repeated(self.repeats));
			self.repeats = 0;
		}
		if self.suppressed > 0 {
			elisions.push(Elision::Suppressed(self.suppressed));
			self.suppressed = 0;
		}
		self.last = Some((key, now));
		Admission::Admitted(elisions)
	}

	fn refill(&mut self, now: Instant) {
		let elapsed = now.saturating_duration_since(self.refilled_at);
		let whole_seconds =
			u32::try_from(elapsed.as_secs()).unwrap_or(u32::MAX);
		if whole_seconds == 0 {
			return;
		}
		let earned = whole_seconds.saturating_mul(SUSTAINED_PER_SECOND);
		self.tokens = self.tokens.saturating_add(earned).min(BURST);
		self.refilled_at += Duration::from_secs(u64::from(whole_seconds));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	fn key(message: &'static str) -> RepeatKey {
		RepeatKey {
			level: DiagnosticLevel::Warn,
			component: DiagnosticComponent::Craft,
			message,
			code: None,
		}
	}

	#[test]
	fn a_burst_is_capped_and_accounted_for_once_it_subsides() {
		let start = Instant::now();
		let mut budget = Budget::new(start);
		let admitted = (0..1000)
			.filter(|index| {
				let record = RepeatKey {
					code: Some(index.to_string()),
					..key("burst")
				};
				budget.admit(record, start) == Admission::Admitted(vec![])
			})
			.count();
		assert_eq!(admitted, usize::try_from(BURST).unwrap());
		let later = start + Duration::from_secs(1);
		assert_eq!(
			budget.admit(key("after"), later),
			Admission::Admitted(vec![Elision::Suppressed(800)])
		);
	}

	#[test]
	fn repeats_collapse_until_something_else_happens() {
		let start = Instant::now();
		let mut budget = Budget::new(start);
		assert_eq!(
			budget.admit(key("same"), start),
			Admission::Admitted(vec![])
		);
		for second in 1..=5 {
			assert_eq!(
				budget.admit(key("same"), start + Duration::from_secs(second)),
				Admission::Suppressed
			);
		}
		assert_eq!(
			budget.admit(key("other"), start + Duration::from_secs(6)),
			Admission::Admitted(vec![Elision::Repeated(5)])
		);
	}

	#[test]
	fn a_repeat_after_the_window_is_written_again() {
		let start = Instant::now();
		let mut budget = Budget::new(start);
		assert_eq!(
			budget.admit(key("same"), start),
			Admission::Admitted(vec![])
		);
		assert_eq!(
			budget.admit(key("same"), start + REPEAT_WINDOW),
			Admission::Admitted(vec![])
		);
	}

	#[test]
	fn different_codes_are_different_records() {
		let start = Instant::now();
		let mut budget = Budget::new(start);
		let one = RepeatKey {
			code: Some("store.unavailable".into()),
			..key("Command failed")
		};
		let two = RepeatKey {
			code: Some("run.not_found".into()),
			..key("Command failed")
		};
		assert_eq!(budget.admit(one, start), Admission::Admitted(vec![]));
		assert_eq!(budget.admit(two, start), Admission::Admitted(vec![]));
	}
}
