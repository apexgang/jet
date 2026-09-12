//! The read: every rule with the matches its interpretation selects
//! today, and what protects each of them.

use super::{
	AutodeleteCandidate, AutodeleteRulePreview, AutodeleteRules,
	CANDIDATE_LIMIT, rule,
};
use crate::{
	ConversationId, Core, CoreError, EventSequence, QueryResult,
	retention::{WorkspaceState, protections},
};

impl Core {
	/// Reads every rule and, for each that has an interpretation, the
	/// Conversations idle at least that long that are not staged, with
	/// whatever protects them. Workspaces are inspected between reads,
	/// outside any transaction, because that takes Git; a Workspace Git
	/// cannot inspect is left out, as the sweep would leave it alone.
	pub(crate) async fn autodelete_rules(
		&self,
	) -> Result<QueryResult, CoreError> {
		let now = self.now_unix_ms();
		let (cursor, records) = self
			.store
			.read(async |tx| {
				Ok::<_, CoreError>((
					EventSequence(tx.event_cursor().await?),
					tx.autodelete_rules().await?,
				))
			})
			.await?;
		let mut rules = Vec::with_capacity(records.len());
		for record in records {
			let rule = self
				.store
				.read(async |tx| rule::load(tx, record).await)
				.await?;
			let candidates = match rule::inactive_days(&rule.state) {
				Some(days) => self.candidates(rule::cutoff(now, days)).await?,
				None => vec![],
			};
			rules.push(AutodeleteRulePreview { rule, candidates });
		}
		Ok(QueryResult::AutodeleteRules(AutodeleteRules {
			cursor,
			rules,
		}))
	}

	async fn candidates(
		&self,
		cutoff: i64,
	) -> Result<Vec<AutodeleteCandidate>, CoreError> {
		let idle = self
			.store
			.read(async |tx| {
				tx.inactive_conversations(cutoff, "", CANDIDATE_LIMIT).await
			})
			.await?;
		let mut candidates = Vec::with_capacity(idle.len());
		for found in idle {
			let conversation_id = ConversationId(found.conversation_id);
			let workspace = self
				.store
				.read(async |tx| tx.workspace_of(conversation_id.0).await)
				.await?;
			let Ok(state) = WorkspaceState::of(workspace.as_ref()).await else {
				continue;
			};
			let found_protections = self
				.store
				.read(async |tx| protections(tx, conversation_id, state).await)
				.await?;
			candidates.push(AutodeleteCandidate {
				conversation_id,
				last_active_at: crate::system_time(
					found.last_active_at_unix_ms,
				),
				protections: found_protections,
			});
		}
		Ok(candidates)
	}
}
