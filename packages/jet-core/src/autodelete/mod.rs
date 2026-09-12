//! Autodelete rules: natural language compiled once into a bounded
//! inactivity predicate that cannot act until its owner has seen the
//! interpretation and approved it (ADR-0015, ADR-0099).
//!
//! The Utility model's only part is the compilation: its output is data
//! validated by the core, and a rule whose compilation was unavailable,
//! ambiguous, or malformed stays refused and authorizes nothing. Editing
//! the source recompiles; editing the interpretation by hand keeps the
//! source and drops the model's reading. Either edit returns the rule to a
//! draft and withdraws any authorization to delete everywhere.
//!
//! The sweep evaluates only approved rules, against Conversations that
//! have been idle at least as long as the rule says and that nothing
//! protects (ADR-0001), and stages what it matches in Jet Trash under the
//! Plane's grace period. Native deletion follows only where the rule was
//! separately authorized to delete everywhere (ADR-0011).

mod command;
mod preview;
mod rule;
mod sweep;

pub(crate) use command::{
	approve, authorize_everywhere, compile, delete, set_inactive_days,
};
pub(crate) use rule::next_deadline;
pub(crate) use sweep::still_matches;
pub use sweep::{AutodeleteMatch, AutodeleteSweep};

use crate::{ConversationId, EventSequence, Protection};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

/// Identity of one Autodelete rule, chosen by the client that compiles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AutodeleteRuleId(pub Uuid);

/// The longest inactivity a rule may name, in whole days: the Utility
/// output schema's own bound (ADR-0099).
pub(crate) const MAX_INACTIVE_DAYS: u32 = 36500;

/// How many candidate matches one rule's preview lists.
pub(crate) const CANDIDATE_LIMIT: i64 = 32;

/// Where a rule stands between compilation and execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
		/// When it was approved.
		approved_at: SystemTime,
	},
}

/// What an approved rule may do with a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct AutodeleteRule {
	/// The rule.
	pub rule_id: AutodeleteRuleId,
	/// Its natural-language source.
	pub prompt: String,
	/// The Utility job that compiled the source, with its full attribution
	/// readable through the Utility Query.
	pub utility_job_id: Uuid,
	/// Where it stands.
	pub state: AutodeleteRuleState,
	/// What its matches are staged for.
	pub scope: AutodeleteScope,
	/// When it was first compiled.
	pub created_at: SystemTime,
	/// When it last changed.
	pub updated_at: SystemTime,
}

/// A Conversation the rule's interpretation selects today, and what would
/// keep the sweep from staging it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutodeleteCandidate {
	/// The idle Conversation.
	pub conversation_id: ConversationId,
	/// When it was created or its latest Run ended, whichever is later.
	pub last_active_at: SystemTime,
	/// What protects it; empty means the sweep would stage it.
	pub protections: Vec<Protection>,
}

/// One rule with the matches its interpretation selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutodeleteRulePreview {
	/// The rule.
	pub rule: AutodeleteRule,
	/// Its candidates, bounded, in identity order; empty while the rule is
	/// compiling or refused.
	pub candidates: Vec<AutodeleteCandidate>,
}

/// Every rule on the Plane, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutodeleteRules {
	/// Event cursor from the same read transaction.
	pub cursor: EventSequence,
	/// The rules, bounded to 64.
	pub rules: Vec<AutodeleteRulePreview>,
}

#[cfg(test)]
pub(super) mod fixtures {
	//! What the Autodelete tests share: reading the rules and issuing a
	//! Command by its outcome or its refusal code.

	use crate::{
		AutodeleteRulePreview, AutodeleteRules, Command, CommandOutcome, Core,
		CoreError, Query, QueryResult,
		test_support::{actor, request},
	};

	pub(crate) async fn rules(core: &Core) -> Vec<AutodeleteRulePreview> {
		let QueryResult::AutodeleteRules(AutodeleteRules { rules, .. }) =
			core.query(&actor(), Query::AutodeleteRules).await.unwrap()
		else {
			panic!("expected the rules");
		};
		rules
	}

	pub(crate) async fn execute(
		core: &Core,
		command: Command,
	) -> Result<CommandOutcome, String> {
		core.execute(&actor(), request(command))
			.await
			.map_err(|error: CoreError| error.code)
	}
}
