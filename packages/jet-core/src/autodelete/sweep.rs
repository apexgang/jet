//! The sweep: approved rules, and only those, against idle unprotected
//! Conversations (ADR-0015).

use super::{AutodeleteRuleId, rule};
use crate::{
	ConversationId, Core, CoreError, RecoveryMode, TrashReason,
	retention::Staging, security::SecurityClass,
};
use jet_store::WriteTransaction;

/// How many idle Conversations one page of the sweep reads.
const SWEEP_PAGE: i64 = 100;

/// One Conversation an approved rule staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutodeleteMatch {
	/// The rule that matched.
	pub rule_id: AutodeleteRuleId,
	/// The Conversation now in Jet Trash.
	pub conversation_id: ConversationId,
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutodeleteSweep {
	/// Conversations staged in Jet Trash, in the order they were staged.
	pub trashed: Vec<AutodeleteMatch>,
}

impl Core {
	/// Stages, for every approved rule, each Conversation idle at least as
	/// long as the rule says that nothing protects, under the Plane's
	/// grace period and with the rule's scope as the reason. Drafts,
	/// refused compilations, and rules still compiling select nothing.
	/// The rule and the Conversation's idleness are read again inside the
	/// transaction that stages, so an edit, a deletion, or a Run that came
	/// and went since the page was read is respected. Nothing runs while
	/// the Plane is in Recovery mode or cannot vouch for its Security
	/// audit (ADR-0105).
	///
	/// # Errors
	///
	/// Returns a store category [`CoreError`] when the store cannot be
	/// read or written.
	pub async fn sweep_autodelete(&self) -> Result<AutodeleteSweep, CoreError> {
		let mut sweep = AutodeleteSweep::default();
		if self.recovery_mode() != RecoveryMode::Serving
			|| self
				.security
				.read()
				.await
				.admit(SecurityClass::Guarded)
				.is_err()
		{
			return Ok(sweep);
		}
		let now = self.now_unix_ms();
		let rules = self
			.store
			.read(async |tx| tx.autodelete_rules().await)
			.await?;
		for record in rules {
			let Some(days) = rule::approved_inactive_days(&record) else {
				continue;
			};
			let rule_id = AutodeleteRuleId(record.rule_id);
			let cutoff = rule::cutoff(now, days);
			let mut after = String::new();
			loop {
				let idle = self
					.store
					.read(async |tx| {
						tx.inactive_conversations(cutoff, &after, SWEEP_PAGE)
							.await
					})
					.await?;
				let Some(last) = idle.last() else {
					break;
				};
				after = last.conversation_id.to_string();
				for found in idle {
					let conversation_id = ConversationId(found.conversation_id);
					let staged = self
						.stage_if_unprotected(
							conversation_id,
							Staging::Autodelete(rule_id),
							now,
						)
						.await?;
					if staged {
						sweep.trashed.push(AutodeleteMatch {
							rule_id,
							conversation_id,
						});
					}
				}
			}
		}
		Ok(sweep)
	}
}

/// Inside the staging transaction: the reason to stage under, if the rule
/// is still approved and the Conversation is still idle past its cutoff.
pub(crate) async fn still_matches(
	tx: &mut WriteTransaction,
	rule_id: AutodeleteRuleId,
	conversation_id: ConversationId,
	now: i64,
) -> Result<Option<TrashReason>, CoreError> {
	let Some(record) = tx.autodelete_rule(rule_id.0).await? else {
		return Ok(None);
	};
	let Some(days) = rule::approved_inactive_days(&record) else {
		return Ok(None);
	};
	let idle = tx
		.conversation_last_active(conversation_id.0)
		.await?
		.is_some_and(|last_active| last_active <= rule::cutoff(now, days));
	Ok(idle.then_some(if record.everywhere {
		TrashReason::AutodeleteEverywhere
	} else {
		TrashReason::AutodeleteRule
	}))
}

#[cfg(test)]
mod tests {
	use super::{AutodeleteMatch, AutodeleteSweep};
	use crate::{
		AuditActor, AutodeleteRuleId, Command, TrashEntry, TrashReason,
		autodelete::fixtures::{execute, rules},
		retention::fixtures::{audit, conversation, trash},
		test_support::{
			FixedProbe, ManualClock, actor, equipped, request, start_core_with,
		},
		workspace::WorkingTreeRequest,
	};
	use jet_store::RetentionPolicy;
	use pretty_assertions::assert_eq;
	use std::{
		sync::Arc,
		time::{Duration, UNIX_EPOCH},
	};
	use uuid::Uuid;

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const DAY: Duration = Duration::from_secs(24 * 60 * 60);

	/// Only an approved rule stages, only what is idle long enough and
	/// unprotected, with the rule's scope as the reason and the Plane's
	/// grace period; a draft of the same interpretation stages nothing.
	#[tokio::test]
	async fn approved_rules_stage_idle_unprotected_matches() {
		let dir = tempfile::tempdir().unwrap();
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await;
		let idle = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		// A Run that never ends keeps this one out of every page.
		let busy = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		core.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: busy,
			}),
		)
		.await
		.unwrap();
		let later = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		clock.advance(20 * DAY);
		// Idle for 11 days by the time the rule asks for 30: not a match.
		conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		clock.advance(11 * DAY);
		let rule_id = AutodeleteRuleId(Uuid::now_v7());
		execute(
			&core,
			Command::CompileAutodeleteRule {
				rule_id,
				prompt: "Forget anything idle for a month".into(),
			},
		)
		.await
		.unwrap();
		core.perform_utilities().await.unwrap();
		execute(
			&core,
			Command::SetAutodeleteRuleInactiveDays {
				rule_id,
				inactive_days: 30,
			},
		)
		.await
		.unwrap();
		let candidates = rules(&core)
			.await
			.into_iter()
			.flat_map(|preview| preview.candidates)
			.map(|candidate| (candidate.conversation_id, candidate.protections))
			.collect::<Vec<_>>();

		let while_draft = core.sweep_autodelete().await.unwrap();
		execute(
			&core,
			Command::ApproveAutodeleteRule {
				rule_id,
				inactive_days: 30,
			},
		)
		.await
		.unwrap();
		let staged = core.sweep_autodelete().await.unwrap();
		let again = core.sweep_autodelete().await.unwrap();
		let first_trash = trash(&core).await;
		execute(&core, Command::AuthorizeAutodeleteEverywhere { rule_id })
			.await
			.unwrap();
		// Restored, `later` is idle and unstaged again, so the next sweep
		// matches it under the rule's widened scope.
		execute(
			&core,
			Command::RestoreConversation {
				conversation_id: later,
			},
		)
		.await
		.unwrap();
		let everywhere = core.sweep_autodelete().await.unwrap();
		let second_trash = trash(&core).await;
		let now = UNIX_EPOCH + NOW + 31 * DAY;

		let mut expected_candidates = vec![(idle, vec![]), (later, vec![])];
		expected_candidates.sort_by_key(|(id, _)| id.0.to_string());
		let mut expected_staged = [idle, later];
		expected_staged.sort_by_key(|id| id.0.to_string());
		assert_eq!(
			(candidates, while_draft, staged.clone(), again),
			(
				expected_candidates,
				AutodeleteSweep::default(),
				AutodeleteSweep {
					trashed: expected_staged
						.iter()
						.map(|&conversation_id| AutodeleteMatch {
							rule_id,
							conversation_id,
						})
						.collect(),
				},
				AutodeleteSweep::default(),
			)
		);
		assert_eq!(
			first_trash,
			expected_staged
				.iter()
				.map(|&conversation_id| TrashEntry {
					conversation_id,
					reason: TrashReason::AutodeleteRule,
					trashed_at: now,
					expires_at: now + 30 * DAY,
				})
				.collect::<Vec<_>>()
		);
		assert_eq!(
			(
				everywhere,
				second_trash
					.iter()
					.map(|entry| (entry.conversation_id, entry.reason))
					.collect::<Vec<_>>(),
			),
			(
				AutodeleteSweep {
					trashed: vec![AutodeleteMatch {
						rule_id,
						conversation_id: later,
					}],
				},
				{
					let mut entries = vec![
						(idle, TrashReason::AutodeleteRule),
						(later, TrashReason::AutodeleteEverywhere),
					];
					entries.sort_by_key(|(id, _)| id.0.to_string());
					entries
				},
			)
		);
		assert_eq!(
			audit(&core)
				.await
				.into_iter()
				.filter(|(_, actor, _)| *actor == AuditActor::Retention)
				.map(|(decision, _, target)| (decision, target))
				.collect::<Vec<_>>(),
			{
				let mut first = expected_staged
					.iter()
					.map(|id| {
						(
							"conversation.forgotten".to_string(),
							Some(id.0.to_string()),
						)
					})
					.collect::<Vec<_>>();
				first.push((
					"conversation.deletion_authorized".into(),
					Some(later.0.to_string()),
				));
				first
			}
		);
	}
}
