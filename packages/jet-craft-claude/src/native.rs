//! What one native event means to the Run, beyond the bytes forwarded as is.
//!
//! Only the few discriminators a Run's lifecycle depends on are read here.
//! Everything else stays opaque and reaches the host as the complete native
//! event, which remains the authoritative payload.
use serde_json::Value;

/// The Run-level meaning of one native event.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Meaning {
	/// The Harness finished the turn it was given and named the native
	/// Conversation that turn belongs to.
	TurnResult {
		/// Native Conversation identity, retained for a later resumed Run.
		native_conversation: String,
	},
	/// The Harness cannot proceed until quota is available again.
	QuotaExhausted,
	/// The Harness began working on the turn it was given, and named the
	/// release it is. An empty release is one the Harness did not report.
	Started {
		/// Claude Code release, checked against the tested matrix.
		version: String,
	},
	/// Nothing beyond the native event itself.
	Opaque,
}

/// Classify one native event. An event this Craft does not recognize is
/// `Opaque`, never an error: the Harness may add events at any time.
pub(crate) fn meaning(event: &Value) -> Meaning {
	match text(event, "type") {
		Some("result") => Meaning::TurnResult {
			native_conversation: text(event, "session_id")
				.unwrap_or_default()
				.to_owned(),
		},
		// A warning still allows the turn to proceed, so only a status the
		// Harness cannot work under is reported as waiting for quota.
		Some("rate_limit_event") => {
			match event
				.pointer("/rate_limit_info/status")
				.and_then(Value::as_str)
			{
				Some(status) if status.starts_with("allowed") => {
					Meaning::Opaque
				}
				Some(_) => Meaning::QuotaExhausted,
				None => Meaning::Opaque,
			}
		}
		Some("system") if text(event, "subtype") == Some("init") => {
			Meaning::Started {
				version: text(event, "claude_code_version")
					.unwrap_or_default()
					.to_owned(),
			}
		}
		Some(_) | None => Meaning::Opaque,
	}
}

fn text<'a>(event: &'a Value, field: &str) -> Option<&'a str> {
	event.get(field).and_then(Value::as_str)
}
