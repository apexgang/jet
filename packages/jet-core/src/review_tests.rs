//! Automatic review against a live managed execution (ADR-0012).

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use crate::checkpoint_tests::{Answered, start_answering};
use crate::test_support::{
	actor, bind_native_account, register_repository, request,
};
use crate::utility_tests::setting;
use crate::{
	AccountBinding, AccountBindingId, ApprovalRequest, ApprovalReview,
	AuditOutcome, AuditRisk, AuditSequence, AutomaticReviewPolicy, Command,
	CommandOutcome, Core, CoreError, Event, EventKind, EventSequence,
	PinnedCraft, ProviderId, Query, QueryResult, RetentionPolicy,
	ReviewAuthorization, ReviewDecision, ReviewHost, ReviewInput,
	ReviewOutcome, ReviewReply, ReviewRisk, ReviewVerdict, Reviewer,
	ReviewerSelection, RunActivity, RunFuture, RunId, RunLifecycle,
	RunObservation, SettingKey, SettingValue, VisaRunRequest,
	WorkingTreeRequest,
};

/// A reviewer that answers with whatever the test told it to, and keeps
/// everything it was shown so a test can assert that nothing else was.
#[derive(Debug)]
struct Judge {
	seen: Mutex<Vec<ReviewInput>>,
	output: Mutex<String>,
	fault: Mutex<Option<Fault>>,
}

#[derive(Debug, Clone)]
enum Fault {
	Failure,
	Timeout,
	WrongReviewer,
	Hold(Arc<tokio::sync::Notify>),
}

impl ReviewHost for Judge {
	fn select<'a>(
		&'a self,
		_craft: &'a PinnedCraft,
		_binding: &'a AccountBinding,
	) -> RunFuture<'a, Result<ReviewerSelection, CoreError>> {
		Box::pin(async {
			Ok(ReviewerSelection {
				reviewer: Reviewer::Equivalent,
				model: "fast-reviewer-v1".into(),
				adapter_state: String::new(),
			})
		})
	}

	fn review<'a>(
		&'a self,
		_craft: &'a PinnedCraft,
		_binding: &'a AccountBinding,
		selection: &'a ReviewerSelection,
		input: &'a ReviewInput,
	) -> RunFuture<'a, Result<ReviewReply, CoreError>> {
		self.seen.lock().expect("seen lock").push(input.clone());
		let fault = self.fault.lock().unwrap().clone();
		Box::pin(async move {
			match &fault {
				Some(Fault::Failure) => {
					return Err(CoreError::conflict(
						"fixture.failed",
						"review failed",
					));
				}
				Some(Fault::Timeout) => return std::future::pending().await,
				Some(Fault::Hold(release)) => release.notified().await,
				Some(Fault::WrongReviewer) | None => {}
			}
			Ok(ReviewReply {
				model: selection.model.clone(),
				reviewer: if matches!(fault, Some(Fault::WrongReviewer)) {
					Reviewer::Native
				} else {
					Reviewer::Equivalent
				},
				output: self.output.lock().unwrap().as_bytes().to_vec(),
			})
		})
	}
}

fn answer(risk: &str, authorization: &str, decision: &str) -> String {
	format!(
		r#"{{"risk":"{risk}","authorization":"{authorization}","decision":"{decision}","rationale":"Judged for the test."}}"#
	)
}

fn asked() -> ApprovalRequest {
	ApprovalRequest {
		request_id: "native-approval-1".into(),
		tool: "Bash".into(),
		action: r#"{"command":"/bin/echo safe"}"#.into(),
	}
}

struct Reviewing {
	core: Arc<Core>,
	sender: mpsc::Sender<RunObservation>,
	answered: Answered,
	judge: Arc<Judge>,
	run_id: RunId,
}

/// Starts a live Run whose reviewer answers `output`, with the Plane policy
/// the test names applied before the Harness asks for anything.
async fn reviewing(
	dir: &std::path::Path,
	output: String,
	policy: impl AsyncFnOnce(Arc<Core>) -> Option<AccountBindingId>,
) -> Reviewing {
	let (core, sender, answered) = start_answering(dir).await;
	let judge = Arc::new(Judge {
		seen: Mutex::default(),
		output: Mutex::new(output),
		fault: Mutex::default(),
	});
	let core = Arc::new(
		Arc::try_unwrap(core)
			.expect("one core reference")
			.with_review_host(Arc::clone(&judge) as Arc<dyn ReviewHost>),
	);
	let binding = policy(Arc::clone(&core)).await;
	let project_id = register_repository(&core, &dir.join("repo")).await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::LocalCheckout { project_id },
			}),
		)
		.await
		.expect("Conversation")
	else {
		panic!("Conversation")
	};
	let QueryResult::Status(status) =
		core.query(&actor(), Query::Status).await.expect("Status")
	else {
		panic!("Status")
	};
	// A Visa Run is the one that carries its own Account binding, which is
	// the reviewer Automatic review uses unless the Plane names another.
	let command = match binding {
		Some(account_binding_id) => Command::StartVisaRun(VisaRunRequest {
			conversation_id: conversation.conversation_id,
			destination_plane_id: status.plane_id,
			account_binding_id,
			craft: "fake".into(),
			prompt: "Run the tests".into(),
		}),
		None => Command::StartRun {
			conversation_id: conversation.conversation_id,
			craft: "fake".into(),
			prompt: "Run the tests".into(),
		},
	};
	let CommandOutcome::RunCreated(run) =
		core.execute(&actor(), request(command)).await.expect("Run")
	else {
		panic!("Run")
	};
	core.perform_runs().await.expect("start");
	wait_for(&core, run.run_id, RunLifecycle::Active).await;
	Reviewing {
		core,
		sender,
		answered,
		judge,
		run_id: run.run_id,
	}
}

impl Reviewing {
	/// Lets the Harness ask, then waits for the review to be recorded.
	async fn ask(&self) -> ApprovalReview {
		self.asks().await;
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				if let Some(review) = self.recorded().await {
					break review;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("a review is recorded")
	}

	/// Lets the Harness ask and waits only for the request itself to commit.
	async fn asks(&self) {
		self.sender
			.send(RunObservation::ApprovalRequested(asked()))
			.await
			.expect("request delivered");
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			while !self.requested().await {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the request is recorded");
	}

	/// Whether the held request itself reached the journal.
	async fn requested(&self) -> bool {
		self.events().await.iter().any(|event| {
			matches!(event.kind, EventKind::ApprovalRequested { .. })
		})
	}

	async fn recorded(&self) -> Option<ApprovalReview> {
		self.events().await.iter().find_map(|event: &Event| {
			match event.kind.clone() {
				EventKind::ApprovalReviewed { review } => Some(*review),
				_ => None,
			}
		})
	}

	async fn events(&self) -> Vec<Event> {
		let QueryResult::Events(page) = self
			.core
			.query(
				&actor(),
				Query::Events {
					after: EventSequence(0),
				},
			)
			.await
			.expect("the journal is readable")
		else {
			panic!("Events")
		};
		page.events
	}

	fn answers(&self) -> Vec<(String, ReviewDecision)> {
		self.answered.lock().expect("answered lock").clone()
	}
}

async fn wait_for(core: &Core, run_id: RunId, lifecycle: RunLifecycle) {
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			if let Ok(QueryResult::RunExecution(state)) =
				core.query(&actor(), Query::RunExecution { run_id }).await
				&& state.run.lifecycle == lifecycle
			{
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("the Run reaches the lifecycle");
}

/// Turns Automatic review on and returns the Run's own binding, so review
/// routes through the Provider the Run already authenticates with.
async fn enabled(core: Arc<Core>) -> Option<AccountBindingId> {
	setting(&core, SettingKey::AutomaticReview, SettingValue::Flag(true)).await;
	Some(bind_native_account(&core, ProviderId("anthropic".into())).await)
}

#[tokio::test]
async fn review_answers_the_exact_request_through_the_runs_own_binding() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing =
		reviewing(dir.path(), answer("low", "sufficient", "allow"), enabled)
			.await;
	let review = reviewing.ask().await;
	assert_eq!(
		(
			review.run_id,
			review.request,
			review.reviewer,
			review.model,
			review.outcome
		),
		(
			reviewing.run_id,
			asked(),
			Some(Reviewer::Equivalent),
			Some("fast-reviewer-v1".into()),
			ReviewOutcome::Decided {
				verdict: ReviewVerdict {
					risk: ReviewRisk::Low,
					authorization: ReviewAuthorization::Sufficient,
					decision: ReviewDecision::Allow,
					rationale: "Judged for the test.".into(),
				},
				decision: ReviewDecision::Allow,
			}
		)
	);
	// Exactly one answer, for exactly the request that was asked.
	assert_eq!(
		reviewing.answers(),
		vec![("native-approval-1".into(), ReviewDecision::Allow)]
	);
	assert_eq!(
		review.policy,
		AutomaticReviewPolicy {
			version: 2,
			cross_provider_consent: false,
		}
	);
}

#[tokio::test]
async fn the_reviewer_sees_only_the_visible_transcript_and_the_exact_action() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing =
		reviewing(dir.path(), answer("low", "sufficient", "allow"), enabled)
			.await;
	reviewing
		.sender
		.send(RunObservation::Output {
			native_json: r#"{"secret":"native only"}"#.into(),
			presentation_json: vec![
				r#"{"kind":"text","text":"Running the tests"}"#.into(),
			],
		})
		.await
		.unwrap();
	// Output a GUI has no portable view of is not the visible transcript,
	// so its lossless native event is shown to no reviewer.
	reviewing
		.sender
		.send(RunObservation::Output {
			native_json: r#"{"tool_result":"the contents of id_ed25519"}"#
				.into(),
			presentation_json: vec![],
		})
		.await
		.unwrap();
	reviewing
		.sender
		.send(RunObservation::Progress {
			offset: 1,
			checkpoint: String::new(),
		})
		.await
		.unwrap();
	reviewing.ask().await;
	let seen = reviewing.judge.seen.lock().expect("seen lock").clone();
	assert_eq!(
		seen,
		vec![ReviewInput {
			transcript: concat!(
				"user: Run the tests\n",
				"harness: {\"kind\":\"text\",\"text\":\"Running the tests\"}"
			)
			.into(),
			tool: "Bash".into(),
			action: r#"{"command":"/bin/echo safe"}"#.into(),
		}]
	);
}

/// The newest output is the one the requested action follows from, so an
/// entry too long for the budget is cut rather than dropped: a reviewer is
/// never asked to judge authorization against an empty transcript.
#[tokio::test]
async fn an_oversized_newest_entry_is_cut_rather_than_emptying_the_transcript()
{
	let dir = tempfile::tempdir().unwrap();
	let reviewing =
		reviewing(dir.path(), answer("low", "sufficient", "allow"), enabled)
			.await;
	reviewing
		.sender
		.send(RunObservation::Output {
			native_json: "{}".into(),
			presentation_json: vec![format!(
				r#"{{"kind":"text","text":"{}"}}"#,
				"x".repeat(20 * 1024)
			)],
		})
		.await
		.unwrap();
	reviewing
		.sender
		.send(RunObservation::Progress {
			offset: 1,
			checkpoint: String::new(),
		})
		.await
		.unwrap();
	reviewing.ask().await;
	let seen = reviewing.judge.seen.lock().expect("seen lock").clone();
	let [only] = seen.as_slice() else {
		panic!("one review")
	};
	assert_eq!(
		(
			only.transcript.len(),
			only.transcript.starts_with(r#"harness: {"kind":"text""#),
		),
		(12 * 1024, true)
	);
}

#[tokio::test]
async fn a_high_risk_action_the_user_never_asked_for_is_denied() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing =
		reviewing(dir.path(), answer("high", "absent", "allow"), enabled).await;
	let review = reviewing.ask().await;
	// The reviewer would have allowed it; the trusted core would not.
	assert_eq!(
		review.outcome,
		ReviewOutcome::Decided {
			verdict: ReviewVerdict {
				risk: ReviewRisk::High,
				authorization: ReviewAuthorization::Absent,
				decision: ReviewDecision::Allow,
				rationale: "Judged for the test.".into(),
			},
			decision: ReviewDecision::Deny,
		}
	);
	assert_eq!(
		reviewing.answers(),
		vec![("native-approval-1".into(), ReviewDecision::Deny)]
	);
}

#[tokio::test]
async fn a_critical_class_is_denied_whatever_the_reviewer_answered() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing = reviewing(
		dir.path(),
		answer("critical", "sufficient", "allow"),
		enabled,
	)
	.await;
	let review = reviewing.ask().await;
	let ReviewOutcome::Decided { decision, .. } = review.outcome else {
		panic!("a decision")
	};
	assert_eq!(
		(decision, reviewing.answers()),
		(
			ReviewDecision::Deny,
			vec![("native-approval-1".into(), ReviewDecision::Deny)]
		)
	);
}

#[tokio::test]
async fn a_cross_provider_reviewer_waits_for_persistent_consent() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing = reviewing(
		dir.path(),
		answer("low", "sufficient", "allow"),
		async |core| {
			setting(
				&core,
				SettingKey::AutomaticReview,
				SettingValue::Flag(true),
			)
			.await;
			// The Plane names a reviewer outside the Run's own Provider and
			// records no consent for it.
			let elsewhere =
				bind_native_account(&core, ProviderId("openai".into())).await;
			setting(
				&core,
				SettingKey::AutomaticReviewBinding,
				SettingValue::Text(elsewhere.0.to_string()),
			)
			.await;
			Some(
				bind_native_account(&core, ProviderId("anthropic".into()))
					.await,
			)
		},
	)
	.await;
	let review = reviewing.ask().await;
	assert_eq!(
		review.outcome,
		ReviewOutcome::Unavailable {
			reason: "review.consent_required".into()
		}
	);
	assert_eq!(reviewing.answers(), vec![]);
	assert!(reviewing.judge.seen.lock().expect("seen lock").is_empty());
}

#[tokio::test]
async fn an_unreadable_reviewer_answer_leaves_the_request_for_a_person() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing = reviewing(
		dir.path(),
		r#"{"decision":"allow","rationale":"no risk field"}"#.into(),
		enabled,
	)
	.await;
	let review = reviewing.ask().await;
	assert_eq!(
		(review.outcome, reviewing.answers()),
		(
			ReviewOutcome::Unavailable {
				reason: "review.output_invalid".into()
			},
			vec![]
		)
	);
}

/// A Plane that never opted in does nothing: it decides nothing, and it
/// records nothing about a request it was never going to answer.
#[tokio::test]
async fn a_plane_that_never_enabled_review_decides_nothing() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing = reviewing(
		dir.path(),
		answer("low", "sufficient", "allow"),
		async |core| {
			Some(
				bind_native_account(&core, ProviderId("anthropic".into()))
					.await,
			)
		},
	)
	.await;
	reviewing.asks().await;
	assert_eq!(
		(reviewing.recorded().await, reviewing.answers()),
		(None, vec![])
	);
	assert!(reviewing.judge.seen.lock().expect("seen lock").is_empty());
}

#[tokio::test]
async fn every_decision_reaches_the_security_audit() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing =
		reviewing(dir.path(), answer("high", "absent", "allow"), enabled).await;
	reviewing.ask().await;
	let QueryResult::SecurityAudit(page) = reviewing
		.core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.expect("audit")
	else {
		panic!("audit")
	};
	let reviewed: Vec<_> = page
		.entries
		.iter()
		.filter(|entry| entry.decision == "approval.reviewed")
		.map(|entry| (entry.target.kind.clone(), entry.risk, entry.outcome))
		.collect();
	assert_eq!(
		reviewed,
		vec![(
			"execution".into(),
			AuditRisk::Elevated,
			AuditOutcome::Denied
		)]
	);
}

/// A Run that carries no Account binding of its own has no reviewer to
/// route to, so the request stays with the person the Harness is waiting on.
#[tokio::test]
async fn a_run_without_a_binding_leaves_the_request_for_a_person() {
	let dir = tempfile::tempdir().unwrap();
	let reviewing = reviewing(
		dir.path(),
		answer("low", "sufficient", "allow"),
		async |core| {
			setting(
				&core,
				SettingKey::AutomaticReview,
				SettingValue::Flag(true),
			)
			.await;
			None
		},
	)
	.await;
	reviewing
		.sender
		.send(RunObservation::Activity(RunActivity::WaitingForApproval))
		.await
		.unwrap();
	let review = reviewing.ask().await;
	assert_eq!(
		(review.binding_id, review.outcome, reviewing.answers()),
		(
			None,
			ReviewOutcome::Unavailable {
				reason: "review.binding_unavailable".into()
			},
			vec![]
		)
	);
}

#[path = "review_guard_tests.rs"]
mod guard_tests;
