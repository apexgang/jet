//! The Commands: compile a source, edit the interpretation by hand, approve
//! it, authorize deleting everywhere, and delete the rule. Each is a
//! Guarded Security decision recorded against the rule (ADR-0105).

use super::{AutodeleteRuleId, AutodeleteRuleState, MAX_INACTIVE_DAYS, rule};
use crate::{
	Actor, CommandId, CommandOutcome, CoreError, UtilityRequest,
	audit::{self, AuditDecision, AuditSubject, Decision},
};
use jet_store::{AutodeleteRuleRecord, WriteTransaction};

/// Compiles `prompt` into the rule `rule_id`: a new rule, or a source edit
/// of an existing one, which returns it to a draft and withdraws any
/// authorization to delete everywhere. The Utility job is admitted here
/// and answered by the worker; until then the rule is compiling.
pub(crate) async fn compile(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	rule_id: AutodeleteRuleId,
	prompt: String,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let existing = tx.autodelete_rule(rule_id.0).await?;
	let utility_job_id = crate::utility::work::admit_job(
		tx,
		command_id,
		UtilityRequest::Autodelete {
			prompt: prompt.clone(),
		},
	)
	.await?;
	let record = AutodeleteRuleRecord {
		rule_id: rule_id.0,
		prompt,
		utility_job_id,
		inactive_days: None,
		approved_at_unix_ms: None,
		everywhere: false,
		created_at_unix_ms: existing
			.map_or(now, |record| record.created_at_unix_ms),
		updated_at_unix_ms: now,
	};
	save_and_record(
		tx,
		actor,
		record,
		AuditDecision::AutodeleteRuleCompiled,
		now,
	)
	.await
}

/// Sets the interpretation by hand, keeping the source. The rule becomes a
/// draft of exactly `inactive_days`, whatever the model said.
pub(crate) async fn set_inactive_days(
	tx: &mut WriteTransaction,
	actor: &Actor,
	rule_id: AutodeleteRuleId,
	inactive_days: u32,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	if !(1..=MAX_INACTIVE_DAYS).contains(&inactive_days) {
		return Err(CoreError::invalid_input(
			"autodelete.inactive_days_out_of_range",
			format!("inactivity is 1 to {MAX_INACTIVE_DAYS} whole days"),
		));
	}
	let record = AutodeleteRuleRecord {
		inactive_days: Some(inactive_days),
		approved_at_unix_ms: None,
		everywhere: false,
		updated_at_unix_ms: now,
		..require(tx, rule_id).await?
	};
	save_and_record(tx, actor, record, AuditDecision::AutodeleteRuleEdited, now)
		.await
}

/// Approves `inactive_days`, the interpretation the owner was shown. A
/// rule whose draft now reads differently, because another client edited
/// or recompiled it in between, is refused rather than approved unseen
/// (ADR-0015). A rule still compiling, one whose compilation was refused,
/// and one already approved are each answered with the state that
/// stopped it.
pub(crate) async fn approve(
	tx: &mut WriteTransaction,
	actor: &Actor,
	rule_id: AutodeleteRuleId,
	inactive_days: u32,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let record = require(tx, rule_id).await?;
	let current = rule::load(tx, record.clone()).await?;
	match current.state {
		AutodeleteRuleState::Draft {
			inactive_days: shown,
		} if shown == inactive_days => {}
		AutodeleteRuleState::Draft {
			inactive_days: shown,
		} => {
			return Err(CoreError::conflict(
				"autodelete.interpretation_changed",
				format!(
					"the draft now reads {shown} days of inactivity, not {inactive_days}; review it again"
				),
			));
		}
		AutodeleteRuleState::Compiling => {
			return Err(CoreError::conflict(
				"autodelete.compiling",
				"the rule has no interpretation to approve yet",
			));
		}
		AutodeleteRuleState::Refused { .. } => {
			return Err(CoreError::conflict(
				"autodelete.refused",
				"compilation produced no interpretation; edit the source or set the inactivity by hand",
			));
		}
		AutodeleteRuleState::Approved { .. } => {
			return Err(CoreError::conflict(
				"autodelete.already_approved",
				"the rule is already approved",
			));
		}
	}
	let record = AutodeleteRuleRecord {
		inactive_days: Some(inactive_days),
		approved_at_unix_ms: Some(now),
		updated_at_unix_ms: now,
		..record
	};
	save_and_record(
		tx,
		actor,
		record,
		AuditDecision::AutodeleteRuleApproved,
		now,
	)
	.await
}

/// Authorizes an approved rule to request native deletion of its matches
/// too, separately from approving what it matches (ADR-0011).
pub(crate) async fn authorize_everywhere(
	tx: &mut WriteTransaction,
	actor: &Actor,
	rule_id: AutodeleteRuleId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let record = require(tx, rule_id).await?;
	if record.approved_at_unix_ms.is_none() {
		return Err(CoreError::conflict(
			"autodelete.not_approved",
			"only an approved rule can be authorized to delete everywhere",
		));
	}
	if record.everywhere {
		return Err(CoreError::conflict(
			"autodelete.already_everywhere",
			"the rule is already authorized to delete everywhere",
		));
	}
	let record = AutodeleteRuleRecord {
		everywhere: true,
		updated_at_unix_ms: now,
		..record
	};
	save_and_record(
		tx,
		actor,
		record,
		AuditDecision::AutodeleteEverywhereAuthorized,
		now,
	)
	.await
}

/// Removes the rule. Conversations it already staged stay in Jet Trash
/// under the reason recorded there; restoring is the way to keep them.
pub(crate) async fn delete(
	tx: &mut WriteTransaction,
	actor: &Actor,
	rule_id: AutodeleteRuleId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	if !tx.delete_autodelete_rule(rule_id.0).await? {
		return Err(not_found());
	}
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::AutodeleteRuleDeleted,
			AuditSubject::AutodeleteRule(rule_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::AutodeleteRuleDeleted { rule_id })
}

async fn require(
	tx: &mut WriteTransaction,
	rule_id: AutodeleteRuleId,
) -> Result<AutodeleteRuleRecord, CoreError> {
	tx.autodelete_rule(rule_id.0).await?.ok_or_else(not_found)
}

fn not_found() -> CoreError {
	CoreError::not_found(
		"autodelete.not_found",
		"the Autodelete rule does not exist",
	)
}

/// Saves `record`, records `decision` about it, and answers with the rule
/// as it now reads.
async fn save_and_record(
	tx: &mut WriteTransaction,
	actor: &Actor,
	record: AutodeleteRuleRecord,
	decision: AuditDecision,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	tx.save_autodelete_rule(&record).await?;
	let rule_id = AutodeleteRuleId(record.rule_id);
	audit::record(
		tx,
		actor,
		Decision::succeeded(decision, AuditSubject::AutodeleteRule(rule_id)),
		now,
	)
	.await?;
	rule::load(tx, record)
		.await
		.map(CommandOutcome::AutodeleteRuleRecorded)
}

#[cfg(test)]
mod tests {
	use crate::{
		AuditActor, AutodeleteRule, AutodeleteRuleId, AutodeleteRulePreview,
		AutodeleteRuleState, AutodeleteScope, Command, CommandOutcome, Core,
		Query, QueryResult, UtilityOutcome,
		autodelete::fixtures::{execute, rules},
		retention::fixtures::{audit, conversation},
		test_support::{
			FixedProbe, ManualClock, actor, equipped, start_core_with,
		},
		utility::tests::{Inference, configured},
		workspace::WorkingTreeRequest,
	};
	use jet_store::RetentionPolicy;
	use pretty_assertions::assert_eq;
	use std::{
		sync::{Arc, Mutex},
		time::{Duration, UNIX_EPOCH},
	};
	use uuid::Uuid;

	const NOW: Duration = Duration::from_millis(1_700_000_000_000);
	const DAY: Duration = Duration::from_secs(24 * 60 * 60);

	async fn started(
		dir: &std::path::Path,
		output: &str,
	) -> (Core, Arc<ManualClock>) {
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&dir.join("plane.sqlite3"),
			Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
			FixedProbe::new(equipped()),
		)
		.await
		.with_utility_host(Arc::new(Inference {
			calls: Mutex::default(),
			output: output.into(),
		}));
		(core, clock)
	}

	fn recorded(outcome: Result<CommandOutcome, String>) -> AutodeleteRule {
		match outcome {
			Ok(CommandOutcome::AutodeleteRuleRecorded(rule)) => rule,
			other => panic!("expected a rule, got {other:?}"),
		}
	}

	/// A prompt compiles into a draft that shows its interpretation and
	/// the Conversations it selects; approval, a hand edit, a
	/// recompilation, and the separate authorization to delete
	/// everywhere each leave the rule exactly where ADR-0015 says: any
	/// edit is a draft again with the authorization withdrawn.
	#[tokio::test]
	async fn a_rule_is_compiled_approved_edited_and_authorized() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) =
			started(dir.path(), r#"{"inactive_days":90}"#).await;
		configured(&core).await;
		let idle = conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		clock.advance(100 * DAY);
		// Created now, so idle for no time at all: never a candidate.
		conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		let rule_id = AutodeleteRuleId(Uuid::now_v7());
		let prompt = "Forget anything idle for three months".to_string();

		let compiling = recorded(
			execute(
				&core,
				Command::CompileAutodeleteRule {
					rule_id,
					prompt: prompt.clone(),
				},
			)
			.await,
		);
		core.perform_utilities().await.unwrap();
		let drafted = rules(&core).await;
		let unseen = execute(
			&core,
			Command::ApproveAutodeleteRule {
				rule_id,
				inactive_days: 60,
			},
		)
		.await;
		let approved = recorded(
			execute(
				&core,
				Command::ApproveAutodeleteRule {
					rule_id,
					inactive_days: 90,
				},
			)
			.await,
		);
		let approved_again = execute(
			&core,
			Command::ApproveAutodeleteRule {
				rule_id,
				inactive_days: 90,
			},
		)
		.await;
		let everywhere = recorded(
			execute(&core, Command::AuthorizeAutodeleteEverywhere { rule_id })
				.await,
		);
		let edited = recorded(
			execute(
				&core,
				Command::SetAutodeleteRuleInactiveDays {
					rule_id,
					inactive_days: 30,
				},
			)
			.await,
		);
		let unapproved_everywhere =
			execute(&core, Command::AuthorizeAutodeleteEverywhere { rule_id })
				.await;
		let out_of_range = execute(
			&core,
			Command::SetAutodeleteRuleInactiveDays {
				rule_id,
				inactive_days: 0,
			},
		)
		.await;
		recorded(
			execute(
				&core,
				Command::ApproveAutodeleteRule {
					rule_id,
					inactive_days: 30,
				},
			)
			.await,
		);
		let recompiled = recorded(
			execute(
				&core,
				Command::CompileAutodeleteRule {
					rule_id,
					prompt: "Forget anything idle for a quarter".into(),
				},
			)
			.await,
		);
		let deleted =
			execute(&core, Command::DeleteAutodeleteRule { rule_id }).await;
		let gone = execute(
			&core,
			Command::ApproveAutodeleteRule {
				rule_id,
				inactive_days: 90,
			},
		)
		.await;
		let now = UNIX_EPOCH + NOW + 100 * DAY;
		let rule = |state, scope, job| AutodeleteRule {
			rule_id,
			prompt: prompt.clone(),
			utility_job_id: job,
			state,
			scope,
			created_at: now,
			updated_at: now,
		};

		assert_eq!(compiling.state, AutodeleteRuleState::Compiling);
		assert_eq!(
			drafted,
			vec![AutodeleteRulePreview {
				rule: rule(
					AutodeleteRuleState::Draft { inactive_days: 90 },
					AutodeleteScope::Forget,
					compiling.utility_job_id,
				),
				candidates: vec![crate::AutodeleteCandidate {
					conversation_id: idle,
					last_active_at: UNIX_EPOCH + NOW,
					protections: vec![],
				}],
			}]
		);
		assert_eq!(
			(unseen, approved, approved_again, everywhere.scope, edited),
			(
				Err("autodelete.interpretation_changed".into()),
				rule(
					AutodeleteRuleState::Approved {
						inactive_days: 90,
						approved_at: now,
					},
					AutodeleteScope::Forget,
					compiling.utility_job_id,
				),
				Err("autodelete.already_approved".into()),
				AutodeleteScope::Everywhere,
				rule(
					AutodeleteRuleState::Draft { inactive_days: 30 },
					AutodeleteScope::Forget,
					compiling.utility_job_id,
				),
			)
		);
		assert_eq!(
			(
				unapproved_everywhere,
				out_of_range,
				recompiled.state,
				recompiled.scope,
				recompiled.utility_job_id == compiling.utility_job_id,
				deleted,
				gone,
				rules(&core).await,
			),
			(
				Err("autodelete.not_approved".into()),
				Err("autodelete.inactive_days_out_of_range".into()),
				AutodeleteRuleState::Compiling,
				AutodeleteScope::Forget,
				false,
				Ok(CommandOutcome::AutodeleteRuleDeleted { rule_id }),
				Err("autodelete.not_found".into()),
				vec![],
			)
		);
		let me = AuditActor::InteractiveClient {
			client_id: actor().client_id(),
		};
		let about_rule = audit(&core)
			.await
			.into_iter()
			.filter(|(decision, ..)| decision.starts_with("autodelete."))
			.collect::<Vec<_>>();
		assert_eq!(
			about_rule,
			[
				"autodelete.rule_compiled",
				"autodelete.rule_approved",
				"autodelete.everywhere_authorized",
				"autodelete.rule_edited",
				"autodelete.rule_approved",
				"autodelete.rule_compiled",
				"autodelete.rule_deleted",
			]
			.into_iter()
			.map(|decision| (
				decision.to_string(),
				me,
				Some(rule_id.0.to_string())
			))
			.collect::<Vec<_>>()
		);
	}

	/// Compilation that is unavailable or answers outside the schema
	/// leaves the rule refused: it shows no interpretation, selects no
	/// candidates, and cannot be approved (ADR-0099).
	#[tokio::test]
	async fn unavailable_or_invalid_compilation_fails_closed() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = started(dir.path(), r#"{"inactive_days":0}"#).await;
		conversation(
			&core,
			RetentionPolicy::Retain,
			WorkingTreeRequest::NoProject,
		)
		.await;
		clock.advance(400 * DAY);
		let disabled = AutodeleteRuleId(Uuid::now_v7());
		execute(
			&core,
			Command::CompileAutodeleteRule {
				rule_id: disabled,
				prompt: "Forget everything old".into(),
			},
		)
		.await
		.unwrap();
		core.perform_utilities().await.unwrap();
		configured(&core).await;
		let invalid = AutodeleteRuleId(Uuid::now_v7());
		let CommandOutcome::AutodeleteRuleRecorded(rule) = execute(
			&core,
			Command::CompileAutodeleteRule {
				rule_id: invalid,
				prompt: "Forget everything old".into(),
			},
		)
		.await
		.unwrap() else {
			panic!("expected a rule");
		};
		core.perform_utilities().await.unwrap();
		let QueryResult::Utility(job) = core
			.query(
				&actor(),
				Query::Utility {
					job_id: rule.utility_job_id,
				},
			)
			.await
			.unwrap()
		else {
			panic!("expected the job");
		};

		assert_eq!(
			(
				rules(&core)
					.await
					.into_iter()
					.map(|preview| (
						preview.rule.rule_id,
						preview.rule.state,
						preview.candidates.len()
					))
					.collect::<Vec<_>>(),
				execute(
					&core,
					Command::ApproveAutodeleteRule {
						rule_id: disabled,
						inactive_days: 1,
					}
				)
				.await,
				execute(
					&core,
					Command::ApproveAutodeleteRule {
						rule_id: invalid,
						inactive_days: 1,
					}
				)
				.await,
				job.outcome,
				core.sweep_autodelete().await.unwrap(),
			),
			(
				vec![
					(
						disabled,
						AutodeleteRuleState::Refused {
							reason: "utility.disabled".into()
						},
						0
					),
					(
						invalid,
						AutodeleteRuleState::Refused {
							reason: "utility.output_invalid".into()
						},
						0
					),
				],
				Err("autodelete.refused".into()),
				Err("autodelete.refused".into()),
				UtilityOutcome::Refused {
					reason: "utility.output_invalid".into()
				},
				crate::AutodeleteSweep::default(),
			)
		);
	}
}
