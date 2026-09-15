//! The bounded, redacted local Diagnostic log of one executable role
//! (ADR-0061).
//!
//! A Diagnostic log is for finding out why the core failed, and nothing
//! else. It is not domain history, which the Event journal holds, and not
//! evidence, which the Security audit holds; nothing here feeds either,
//! and nothing here leaves the machine. Its records are shaped so that
//! what must never be persisted cannot be: a message is a static template,
//! the structured fields carry counts, identities and stable codes, and
//! the one free-text field, a failure's own explanation, is scrubbed and
//! bounded in `redaction`. Volume is bounded twice over: `budget` limits
//! how many records a burst or a repeated failure may write, and
//! `rotation` keeps the files to five of five MiB each.
//!
//! Debug records exist for a person chasing one problem and are kept only
//! while that person has asked for them; a log opened without
//! [`DebugLogging::Enabled`] drops them unread.

mod budget;
mod redaction;
mod rotation;

use budget::{Admission, Budget, Elision, RepeatKey};
use rotation::Ring;
use serde::Serialize;
use std::{
	fmt::Display,
	io::{self, Write},
	path::Path,
	sync::{Arc, Mutex, OnceLock},
	time::{Instant, SystemTime},
};

/// How many structured fields one record may carry.
const FIELD_LIMIT: usize = 8;

/// Which executable is writing. Each role rotates its own files, so a
/// noisy helper cannot push the daemon's history out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableRole {
	/// `jetd`, the Plane daemon.
	Daemon,
}

impl ExecutableRole {
	fn name(self) -> &'static str {
		match self {
			Self::Daemon => "jetd",
		}
	}
}

/// Whether the person running the executable asked for debug records.
/// They are never on by default (ADR-0061).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugLogging {
	/// Debug records are dropped.
	Disabled,
	/// Debug records are kept, because the user explicitly turned them on
	/// for this process.
	Enabled,
}

/// How much a record matters to somebody reading the log later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
	/// Detail wanted only while chasing one problem.
	Debug,
	/// Something expected happened that is worth a line.
	Info,
	/// Something failed and was recovered from or retried.
	Warn,
	/// Something failed and the executable could not carry on as before.
	Error,
}

/// Which part of the executable a record is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticComponent {
	/// Process lifetime: start, stop, and what stopped it.
	Process,
	/// Client connections and the protocol spoken on them.
	Connection,
	/// Pairing and the authentication of remote clients.
	Authentication,
	/// Craft supervision, installation, and lifecycle.
	Craft,
	/// Harness-native extension changes.
	Extension,
	/// Managed Runs and their executions.
	Run,
	/// Workspace terminals.
	Terminal,
	/// Workspace promotions and working trees.
	Workspace,
	/// Store recovery, snapshots, and Recovery bundles.
	Recovery,
	/// Retention, Autodelete, schedules, and other maintenance.
	Maintenance,
	/// Utility work and Git delivery.
	Utility,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum Field {
	Count(u64),
	Text(String),
}

/// One record on its way into the log. The message is a static template
/// by construction; anything that varies goes through a typed field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
	level: DiagnosticLevel,
	component: DiagnosticComponent,
	message: &'static str,
	/// The stable code of what failed, kept apart from the other fields
	/// because it also tells repeats of one failure from another.
	code: Option<String>,
	fields: Vec<(&'static str, Field)>,
}

impl Diagnostic {
	/// A record wanted only while chasing one problem.
	#[must_use]
	pub fn debug(
		component: DiagnosticComponent,
		message: &'static str,
	) -> Self {
		Self::new(DiagnosticLevel::Debug, component, message)
	}

	/// A record of something expected and worth a line.
	#[must_use]
	pub fn info(component: DiagnosticComponent, message: &'static str) -> Self {
		Self::new(DiagnosticLevel::Info, component, message)
	}

	/// A record of a failure that was recovered from or will be retried.
	#[must_use]
	pub fn warn(component: DiagnosticComponent, message: &'static str) -> Self {
		Self::new(DiagnosticLevel::Warn, component, message)
	}

	/// A record of a failure the executable could not carry on from.
	#[must_use]
	pub fn error(
		component: DiagnosticComponent,
		message: &'static str,
	) -> Self {
		Self::new(DiagnosticLevel::Error, component, message)
	}

	fn new(
		level: DiagnosticLevel,
		component: DiagnosticComponent,
		message: &'static str,
	) -> Self {
		Self {
			level,
			component,
			message,
			code: None,
			fields: Vec::new(),
		}
	}

	/// The stable code of what failed, such as `store.unavailable`.
	#[must_use]
	pub fn code(mut self, code: &str) -> Self {
		self.code = Some(redaction::sanitize_code(code));
		self
	}

	/// How many of something there were.
	#[must_use]
	pub fn count(self, name: &'static str, count: u64) -> Self {
		self.with(name, Field::Count(count))
	}

	/// The identity of what the record is about, such as a Run or
	/// Conversation identifier. Names and other content are not identities.
	#[must_use]
	pub fn identity(self, name: &'static str, identity: &dyn Display) -> Self {
		self.with(
			name,
			Field::Text(redaction::sanitize_code(&identity.to_string())),
		)
	}

	/// What a failure said about itself, scrubbed and bounded before it is
	/// kept: this is the one place prose enters the log.
	#[must_use]
	pub fn failure(self, error: &dyn Display) -> Self {
		self.with(
			"failure",
			Field::Text(redaction::sanitize_failure(&error.to_string())),
		)
	}

	fn with(mut self, name: &'static str, field: Field) -> Self {
		if self.fields.len() < FIELD_LIMIT {
			self.fields.push((name, field));
		}
		self
	}

	/// Writes the record to the log this process installed, or drops it
	/// when there is none.
	pub fn emit(self) {
		if let Some(log) = INSTALLED.get() {
			log.record(self);
		}
	}

	fn key(&self) -> RepeatKey {
		RepeatKey {
			level: self.level,
			component: self.component,
			message: self.message,
			code: self.code.clone(),
		}
	}
}

#[derive(Serialize)]
struct Record {
	time: String,
	level: DiagnosticLevel,
	component: DiagnosticComponent,
	message: &'static str,
	#[serde(skip_serializing_if = "serde_json::Map::is_empty")]
	fields: serde_json::Map<String, serde_json::Value>,
}

static INSTALLED: OnceLock<DiagnosticLog> = OnceLock::new();

/// The open Diagnostic log of one executable role. Cheap to clone; every
/// clone writes through the same budget and file ring.
#[derive(Debug, Clone)]
pub struct DiagnosticLog {
	inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
	/// `None` once a write has failed: the log then drops everything
	/// rather than risk the executable on its account.
	ring: Option<Ring>,
	budget: Budget,
	debug: DebugLogging,
	role: ExecutableRole,
}

impl DiagnosticLog {
	/// Opens the log of `role` under `directory`, creating the owner-only
	/// directory and file when missing.
	///
	/// # Errors
	///
	/// Returns the I/O error when the directory or the live file cannot be
	/// created owner-only. The caller carries on without a log; a Plane is
	/// never refused on its diagnostics' account.
	pub fn open(
		directory: &Path,
		role: ExecutableRole,
		debug: DebugLogging,
	) -> io::Result<Self> {
		let ring = Ring::open(directory, role.name())?;
		Ok(Self {
			inner: Arc::new(Mutex::new(Inner {
				ring: Some(ring),
				budget: Budget::new(Instant::now()),
				debug,
				role,
			})),
		})
	}

	/// Makes this the log [`Diagnostic::emit`] writes to for the rest of
	/// the process. A second installation is ignored.
	pub fn install(self) {
		let _ = INSTALLED.set(self);
	}

	/// Writes one record, subject to the level, the budget, and the ring.
	/// Never fails and never blocks on anything but the log's own lock.
	fn record(&self, diagnostic: Diagnostic) {
		self.record_at(diagnostic, Instant::now(), SystemTime::now());
	}

	fn record_at(
		&self,
		diagnostic: Diagnostic,
		now: Instant,
		time: SystemTime,
	) {
		let Ok(mut inner) = self.inner.lock() else {
			return;
		};
		if diagnostic.level == DiagnosticLevel::Debug
			&& inner.debug == DebugLogging::Disabled
		{
			return;
		}
		let elisions = match inner.budget.admit(diagnostic.key(), now) {
			Admission::Admitted(elisions) => elisions,
			Admission::Suppressed => return,
		};
		for elision in elisions {
			let account = match elision {
				Elision::Repeated(count) => Diagnostic::info(
					diagnostic.component,
					"earlier record repeated",
				)
				.count("times", count),
				Elision::Suppressed(count) => Diagnostic::info(
					diagnostic.component,
					"records suppressed by budget",
				)
				.count("records", count),
			};
			inner.write(&account, time);
		}
		inner.write(&diagnostic, time);
	}
}

impl Inner {
	fn write(&mut self, diagnostic: &Diagnostic, time: SystemTime) {
		let Some(ring) = &mut self.ring else {
			return;
		};
		let record = Record {
			time: rfc3339_millis(time),
			level: diagnostic.level,
			component: diagnostic.component,
			message: diagnostic.message,
			fields: diagnostic
				.code
				.iter()
				.map(|code| ("code", serde_json::Value::from(code.as_str())))
				.chain(diagnostic.fields.iter().filter_map(|(name, field)| {
					Some((*name, serde_json::to_value(field).ok()?))
				}))
				.map(|(name, value)| (name.to_owned(), value))
				.collect(),
		};
		let Ok(mut line) = serde_json::to_vec(&record) else {
			return;
		};
		line.push(b'\n');
		if let Err(error) = ring.append(&line) {
			// The one line the log writes about itself goes to stderr, once,
			// and a closed stderr is no reason to panic under the lock.
			let _ = writeln!(
				io::stderr(),
				"{}: the Diagnostic log stopped and drops records: {error}",
				self.role.name()
			);
			self.ring = None;
		}
	}
}

/// `time` as RFC 3339 in UTC with millisecond precision, the form every
/// diagnostic record carries. Written here because the release envelope in
/// `release.toml` forbids `jetfueld` and the Crafts a calendar crate; times
/// before the Unix epoch are clamped to it.
fn rfc3339_millis(time: std::time::SystemTime) -> String {
	let since_epoch = time
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default();
	let seconds = since_epoch.as_secs();
	let millis = since_epoch.subsec_millis();
	let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
	let second_of_day = seconds % 86_400;
	// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
	let z = days + 719_468;
	let era = z.div_euclid(146_097);
	let day_of_era = z.rem_euclid(146_097);
	let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524
		- day_of_era / 146_096)
		/ 365;
	let day_of_year =
		day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
	let month_index = (5 * day_of_year + 2) / 153;
	let day = day_of_year - (153 * month_index + 2) / 5 + 1;
	let month = if month_index < 10 {
		month_index + 3
	} else {
		month_index - 9
	};
	let year = year_of_era + era * 400 + i64::from(month <= 2);
	format!(
		"{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
		second_of_day / 3_600,
		second_of_day % 3_600 / 60,
		second_of_day % 60
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;
	use serde_json::{Value, json};
	use std::time::Duration;

	fn lines(directory: &Path) -> Vec<Value> {
		std::fs::read_to_string(directory.join("jetd.log"))
			.unwrap()
			.lines()
			.map(|line| {
				let mut value: Value = serde_json::from_str(line).unwrap();
				let time = value["time"].as_str().unwrap();
				assert!(time.ends_with('Z'), "{time}");
				value.as_object_mut().unwrap().remove("time");
				value
			})
			.collect()
	}

	#[test]
	fn records_are_one_redacted_json_line_each() {
		let dir = tempfile::tempdir().unwrap();
		let log = DiagnosticLog::open(
			dir.path(),
			ExecutableRole::Daemon,
			DebugLogging::Disabled,
		)
		.unwrap();
		log.record(
			Diagnostic::warn(
				DiagnosticComponent::Authentication,
				"pairing refused",
			)
			.code("pairing.rejected")
			.identity("client", &"7a1d5e2c-3b4f-4c5d-8e6f-0a1b2c3d4e5f")
			.count("attempts", 3)
			.failure(&"bad secret=abc123 in {\"prompt\":\"hi\"}"),
		);
		log.record(Diagnostic::debug(DiagnosticComponent::Process, "unwanted"));
		assert_eq!(
			lines(dir.path()),
			vec![json!({
				"level": "warn",
				"component": "authentication",
				"message": "pairing refused",
				"fields": {
					"code": "pairing.rejected",
					"client": "7a1d5e2c-3b4f-4c5d-8e6f-0a1b2c3d4e5f",
					"attempts": 3,
					"failure": "bad secret=[redacted] in [payload redacted]",
				},
			})]
		);
	}

	#[test]
	fn debug_records_need_explicit_activation() {
		let dir = tempfile::tempdir().unwrap();
		let log = DiagnosticLog::open(
			dir.path(),
			ExecutableRole::Daemon,
			DebugLogging::Enabled,
		)
		.unwrap();
		log.record(Diagnostic::debug(DiagnosticComponent::Process, "wanted"));
		assert_eq!(
			lines(dir.path()),
			vec![json!({
				"level": "debug",
				"component": "process",
				"message": "wanted",
			})]
		);
	}

	#[test]
	fn repeated_failures_are_counted_not_written() {
		let dir = tempfile::tempdir().unwrap();
		let log = DiagnosticLog::open(
			dir.path(),
			ExecutableRole::Daemon,
			DebugLogging::Disabled,
		)
		.unwrap();
		let start = Instant::now();
		let time = SystemTime::UNIX_EPOCH;
		for second in 0..100 {
			log.record_at(
				Diagnostic::warn(
					DiagnosticComponent::Craft,
					"cannot reconcile Crafts",
				)
				.code("store.unavailable"),
				start + Duration::from_secs(second),
				time,
			);
		}
		log.record_at(
			Diagnostic::info(DiagnosticComponent::Craft, "reconciled"),
			start + Duration::from_secs(100),
			time,
		);
		let failure = json!({
			"level": "warn",
			"component": "craft",
			"message": "cannot reconcile Crafts",
			"fields": {"code": "store.unavailable"},
		});
		let repeated = |times: u64| {
			json!({
				"level": "info",
				"component": "craft",
				"message": "earlier record repeated",
				"fields": {"times": times},
			})
		};
		assert_eq!(
			lines(dir.path()),
			vec![
				failure.clone(),
				repeated(59),
				failure,
				repeated(39),
				json!({
					"level": "info",
					"component": "craft",
					"message": "reconciled",
				}),
			]
		);
	}

	#[test]
	fn the_log_is_refused_where_it_cannot_be_owner_only() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("not-a-directory");
		std::fs::write(&file, b"").unwrap();
		assert!(
			DiagnosticLog::open(
				&file,
				ExecutableRole::Daemon,
				DebugLogging::Disabled
			)
			.is_err()
		);
	}

	/// The record's time is RFC 3339 in UTC with milliseconds, across a
	/// leap day, a century boundary, and the epoch itself.
	#[test]
	fn diagnostic_times_are_rfc3339_utc_with_milliseconds() {
		let at = |seconds: u64, millis: u32| {
			rfc3339_millis(
				std::time::UNIX_EPOCH
					+ std::time::Duration::new(seconds, millis * 1_000_000),
			)
		};
		assert_eq!(
			[
				at(0, 0),
				at(951_782_400, 7),
				at(4_102_444_799, 999),
				at(1_789_487_045, 120)
			],
			[
				"1970-01-01T00:00:00.000Z".to_owned(),
				"2000-02-29T00:00:00.007Z".to_owned(),
				"2099-12-31T23:59:59.999Z".to_owned(),
				"2026-09-15T15:44:05.120Z".to_owned(),
			]
		);
	}
}
