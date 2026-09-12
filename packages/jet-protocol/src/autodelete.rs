//! Autodelete rules: natural language compiled once into a bounded
//! inactivity predicate that executes only after approval (ADR-0015,
//! ADR-0099). Introduced by protocol minor 40.

use crate::retention::RetentionProtection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a rule stands between compilation and execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AutodeleteRuleState {
	/// The Utility job has not answered yet.
	Compiling,
	/// Compilation produced no usable draft; the rule authorizes nothing
	/// until its source or interpretation is edited.
	Refused {
		/// The stable, content-free reason the Utility job recorded.
		reason: String,
	},
	/// An interpretation awaits approval. Nothing executes it.
	Draft {
		/// Whole days a Conversation must have been idle to match.
		inactive_days: u32,
	},
	/// The owner approved exactly this interpretation; the sweep evaluates
	/// it.
	Approved {
		/// Whole days a Conversation must have been idle to match.
		inactive_days: u32,
		/// When it was approved, in signed Unix milliseconds.
		approved_at_unix_ms: i64,
	},
}

/// What an approved rule may do with a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AutodeleteScope {
	/// Forget: Jet-owned state goes, native history stays.
	Forget,
	/// Also request native deletion when the grace period ends, where the
	/// Harness supports that. Needs its own authorization.
	Everywhere,
}

/// One rule as the Plane holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutodeleteRule {
	/// The rule, chosen by the client that compiled it.
	pub rule_id: Uuid,
	/// Its natural-language source.
	pub prompt: String,
	/// The Utility job that compiled the source; its attribution is
	/// readable through the `utility` Query.
	pub utility_job_id: Uuid,
	/// Where it stands.
	pub state: AutodeleteRuleState,
	/// What its matches are staged for.
	pub scope: AutodeleteScope,
	/// When it was first compiled, in signed Unix milliseconds.
	pub created_at_unix_ms: i64,
	/// When it last changed.
	pub updated_at_unix_ms: i64,
}

/// A Conversation the rule's interpretation selects today, and what would
/// keep the sweep from staging it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutodeleteCandidate {
	/// The idle Conversation.
	pub conversation_id: Uuid,
	/// When it was created or its latest Run ended, whichever is later.
	pub last_active_at_unix_ms: i64,
	/// What protects it, in a fixed order; empty means the sweep would
	/// stage it.
	pub protections: Vec<RetentionProtection>,
}

/// One rule with the matches its interpretation selects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutodeleteRulePreview {
	/// The rule.
	pub rule: AutodeleteRule,
	/// Its candidates, bounded to 32, in identity order; empty while the
	/// rule is compiling or refused.
	pub candidates: Vec<AutodeleteCandidate>,
}

/// Every rule on the Plane, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutodeleteRules {
	/// Plane Event high-water cursor in this read transaction.
	#[serde(with = "crate::transport::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub cursor: u64,
	/// The rules, bounded to 64.
	pub rules: Vec<AutodeleteRulePreview>,
}
