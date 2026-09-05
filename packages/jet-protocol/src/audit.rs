//! Wire form of the owner-only Security audit (ADR-0105).
//!
//! An audit record says who decided what about which target, how risky the
//! decision was, and how it turned out. It carries no credential, prompt,
//! terminal output, or file content, and there is no field on this side of
//! the seam able to hold one.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::Actor;

/// How much a decision could cost if it was not the one the owner intended,
/// as the Plane judged it when the decision was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditRisk {
	/// Recorded so it can be reviewed; it widens nothing and destroys
	/// nothing.
	Routine,
	/// Widens trust, changes policy, or exposes state.
	Elevated,
	/// May destroy state that cannot be brought back from within Jet.
	Destructive,
}

/// What became of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
	/// It was carried out.
	Succeeded,
	/// It was refused before anything changed.
	Denied,
	/// It was allowed but did not complete.
	Failed,
}

/// What one recorded decision was about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditTarget {
	/// The kind of thing it was about, such as `account_binding`. A client
	/// that does not know a kind shows the record generically (ADR-0094).
	pub kind: String,
	/// Lowercase hexadecimal of the opaque identifier the audit's integrity
	/// chain covers. It outlives the target, so records about one thing
	/// still group together after that thing is deleted.
	pub reference: String,
	/// The target's own identity, while the Plane still keeps the target.
	/// Deleting the target leaves only `reference`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub identity: Option<String>,
}

/// One recorded decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
	/// Plane-local audit position, carried as a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub sequence: u64,
	/// The authority epoch the record belongs to, carried as a decimal
	/// string. It changes only when an owner explicitly carries on past an
	/// integrity failure.
	#[serde(with = "crate::decimal")]
	pub epoch: u64,
	/// Durable identity.
	pub record_id: Uuid,
	/// When the decision was made, in signed Unix milliseconds.
	pub recorded_at_unix_ms: i64,
	/// The Plane that made it.
	pub plane_id: Uuid,
	/// The authenticated Actor it is attributed to.
	pub actor: Actor,
	/// What it was about.
	pub target: AuditTarget,
	/// What was decided, such as `account.bound`.
	pub decision: String,
	/// How much it could cost.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
}

/// One page of the Security audit, fenced by the position the audit had
/// reached when the page was read. The page is the last one when its final
/// record's sequence equals `cursor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityAudit {
	/// Newest audit position when the page was read, carried as a decimal
	/// string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The records strictly after the requested position, oldest first.
	pub entries: Vec<AuditEntry>,
}
