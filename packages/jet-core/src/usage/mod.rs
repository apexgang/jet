//! Normalized Usage records: what a Craft may report, and what a Query
//! answers with (ADR-0023, ADR-0045).
//!
//! Two things are measured here and they are never mixed. A Provider
//! reports how full a quota window is; Jet observes what a Harness said it
//! consumed. Each record carries where it came from, what it covers,
//! whether Jet measured or estimated it, whether it can still change, and
//! the Account binding, Model, Conversation, Run, Plane, and time it
//! belongs to.
//!
//! Nothing here claims more than one Plane knows. A Plane answers for its
//! own bindings; assembling connected Planes into one Provider account is
//! the GUI's claim to make, and it can only make it about the Planes it is
//! actually connected to (ADR-0016).

pub(crate) mod query;
pub(crate) mod record;

use crate::{
	AccountBindingId, ConversationId, EventSequence, PlaneId, ProviderId, RunId,
};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// The interval ADR-0045 bounds an idle refresh by. A Provider that has
/// not answered within it is no longer describing what it would say now,
/// and an unchanged answer inside it is a heartbeat rather than another
/// stored snapshot.
pub(crate) const USAGE_REFRESH_MS: i64 = 15 * 60 * 1000;

/// Longest Provider window name, Model name, or unavailability reason a
/// record carries. Each is bounded metadata, not a payload.
pub(crate) const MAX_USAGE_TEXT: usize = 128;

/// Longest reason text an unreachable Provider report carries.
pub(crate) const MAX_REASON_TEXT: usize = 256;

/// An inference model made available through a Provider account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(pub String);

/// Where one Usage record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
	/// A Provider's own accounting of one of its quota windows.
	ProviderQuota,
	/// Jet's own accounting of what a Harness reported it consumed.
	JetObserved,
	/// A Provider that did not answer for one of its windows.
	ProviderUnreachable,
}

/// Whether Jet measured a record's numbers or derived them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEstimation {
	/// Reported by the Harness or the Provider.
	Measured,
	/// Derived by Jet, and never presented as a Provider's own accounting.
	Estimated,
}

/// Whether a record's numbers can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFinality {
	/// The work it covers had not finished when it was reported.
	Interim,
	/// The work it covers is over and the numbers no longer move.
	Final,
}

/// The token counts one Jet-observed measurement carries. Tokens a
/// Provider wrote to its cache count as input; the Harness's own
/// vocabulary stays in the journalled native event beside this record.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub struct UsageTokens {
	/// Tokens sent.
	pub input: u64,
	/// Tokens served from the Provider's cache.
	pub cached_input: u64,
	/// Tokens generated.
	pub output: u64,
	/// Tokens spent on reasoning, where the Provider counts them apart.
	pub reasoning: u64,
}

impl UsageTokens {
	/// The two counts added together, saturating rather than wrapping: a
	/// total no counter could reach is still not a wrong smaller one.
	#[must_use]
	pub fn saturating_add(self, other: Self) -> Self {
		Self {
			input: self.input.saturating_add(other.input),
			cached_input: self.cached_input.saturating_add(other.cached_input),
			output: self.output.saturating_add(other.output),
			reasoning: self.reasoning.saturating_add(other.reasoning),
		}
	}
}

/// What one Jet-observed measurement covers, and what makes it one
/// measurement rather than another. Repeating a measurement replaces it;
/// nothing is added to a number that already covers it (ADR-0023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageMeasurement {
	/// One turn, identified by the Harness's own usage identity when it
	/// supplies one and by the turn it covers otherwise.
	Turn {
		/// The turn the Harness was working on.
		turn: String,
		/// The Harness's or Provider's own identity for the measurement.
		native_usage_id: Option<String>,
	},
	/// The Run so far, as a cumulative total the Harness restates. A Run
	/// that reports these does not also have its turns added in.
	Run {
		/// The Harness's or Provider's own identity for the measurement.
		native_usage_id: Option<String>,
	},
}

/// One Jet-observed consumption measurement as a Craft reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedUsage {
	/// What it covers and what identifies it.
	pub measurement: UsageMeasurement,
	/// The Model that did the work, when the Harness names one.
	pub model: Option<ModelId>,
	/// Whether the Harness measured the counts or Jet derived them.
	pub estimation: UsageEstimation,
	/// Whether the counts can still change.
	pub finality: UsageFinality,
	/// The counts themselves.
	pub tokens: UsageTokens,
}

/// The unit a Provider stated one window in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
	/// Inference tokens.
	Tokens,
	/// Requests.
	Requests,
	/// Provider-defined credits.
	Credits,
	/// Hundredths of a percent of the window, out of 10,000. It is what a
	/// Provider that reports a filled fraction rather than a countable
	/// limit supplies.
	Share,
}

/// How full a Provider says one window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaMeasure {
	/// The unit the Provider stated it in.
	pub unit: QuotaUnit,
	/// How much of the window it reported as consumed.
	pub used: u64,
	/// The limit it stated, where it stated one. A share always has
	/// 10,000.
	pub limit: Option<u64>,
}

/// What one Provider-reported quota window covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaScope {
	/// The Provider account as a whole.
	ProviderAccount,
	/// One Model of that account.
	Model(ModelId),
}

/// One Provider-reported quota window as a Craft reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaReport {
	/// The Provider's own name for the window, such as its five-hour or
	/// weekly limit. Windows are read freshest-first and never summed
	/// into one another.
	pub window: String,
	/// What the window covers.
	pub scope: QuotaScope,
	/// How full the Provider says it is.
	pub measure: QuotaMeasure,
	/// How long the window lasts, where the Provider states it.
	pub window_seconds: Option<u64>,
	/// How long until it refills, where the Provider states it. The Plane
	/// converts it with its own clock, so a Craft never asserts a time.
	pub resets_in_seconds: Option<u64>,
	/// Whether the Provider measured it or Jet derived it.
	pub estimation: UsageEstimation,
	/// Whether the window has closed.
	pub finality: UsageFinality,
}

/// What a Craft reports about the Usage of one execution (ADR-0023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageReport {
	/// What the Harness said it consumed.
	Observed(ObservedUsage),
	/// A quota window the Provider reported.
	ProviderQuota(QuotaReport),
	/// A Provider that would not report its windows. The Plane says so
	/// rather than presenting the last windows it saw as current.
	ProviderUnreachable {
		/// Bounded, non-secret text naming why.
		reason: String,
	},
}

/// Whether a Provider-reported window still stands for what the Provider
/// would say now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageFreshness {
	/// Read recently enough to stand for the Provider's current state.
	Fresh,
	/// Older than the interval a Plane refreshes on. It is history, and
	/// never a current reading (ADR-0045).
	Stale,
	/// The Provider did not answer the last time the Plane asked.
	Unreachable {
		/// Why it did not answer, as the Craft reported it.
		reason: String,
	},
}

/// One Provider-reported quota window, as the freshest response about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindow {
	/// The Plane-local Account binding it belongs to.
	pub binding_id: AccountBindingId,
	/// The Provider that reported it.
	pub provider: ProviderId,
	/// The Provider's own name for the window.
	pub window: String,
	/// What the window covers.
	pub scope: QuotaScope,
	/// The Conversation the response was observed in.
	pub conversation_id: Option<ConversationId>,
	/// The Run the response was observed in.
	pub run_id: Option<RunId>,
	/// How full the Provider said it was.
	pub measure: QuotaMeasure,
	/// How long the window lasts, where the Provider stated it.
	pub window_seconds: Option<u64>,
	/// When it refills, where the Provider stated it.
	pub resets_at: Option<SystemTime>,
	/// Whether the Provider measured it or Jet derived it.
	pub estimation: UsageEstimation,
	/// Whether the window has closed.
	pub finality: UsageFinality,
	/// When the Plane observed the response.
	pub observed_at: SystemTime,
	/// Whether it still stands for what the Provider would say now.
	pub freshness: UsageFreshness,
}

/// Deduplicated Jet-observed consumption for one Model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConsumption {
	/// The Model, absent where the Harness named none.
	pub model: Option<ModelId>,
	/// The counts.
	pub tokens: UsageTokens,
	/// How many deduplicated measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated rather than measured.
	pub estimated: u64,
	/// How many of them can still change.
	pub interim: u64,
	/// When the newest contributing measurement was observed.
	pub last_observed_at: SystemTime,
}

/// Deduplicated Jet-observed consumption for one selection, with the
/// uncertainty in it left visible rather than folded into the total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedConsumption {
	/// The counts across every Model in the selection.
	pub tokens: UsageTokens,
	/// How many deduplicated measurements contributed.
	pub measurements: u64,
	/// How many of them Jet estimated rather than measured.
	pub estimated: u64,
	/// How many of them can still change.
	pub interim: u64,
	/// When the newest contributing measurement was observed.
	pub last_observed_at: Option<SystemTime>,
	/// The same consumption per Model.
	pub models: Vec<ModelConsumption>,
}

/// What one Usage Query covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSelection {
	/// Every Account binding and Conversation on this Plane.
	Plane,
	/// One Plane-local Account binding.
	Binding(AccountBindingId),
	/// One Conversation. Quota windows belong to an Account binding rather
	/// than to a Conversation, so this selection answers with consumption.
	Conversation(ConversationId),
	/// One Run, answered the same way as its Conversation.
	Run(RunId),
}

/// What one Plane knows about Usage for the selected scope, fenced by the
/// journal position the snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneUsage {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The Plane every record here was observed on. A total covers this
	/// Plane alone: no Plane knows what another one consumed, so none of
	/// them answers for a Provider account as a whole (ADR-0016).
	pub plane_id: PlaneId,
	/// The freshest Provider response about each window of each selected
	/// Account binding. Two windows are never added together.
	pub quota_windows: Vec<QuotaWindow>,
	/// Deduplicated Jet-observed consumption for the selection.
	pub consumption: ObservedConsumption,
}

#[cfg(test)]
pub(crate) mod tests {
	use std::sync::Arc;
	use std::time::{Duration, UNIX_EPOCH};

	use jet_store::{
		ConversationOriginRecord, NewConversation, NewRun, WorkingTreeRecord,
	};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::test_support::{
		FixedProbe, ManualClock, actor, equipped, events, request,
		start_core_with,
	};
	use crate::usage::{
		ObservedUsage, QuotaMeasure, QuotaReport, QuotaScope, QuotaUnit,
		UsageEstimation, UsageFinality, UsageFreshness, UsageMeasurement,
		UsageReport, UsageSelection, UsageSource, UsageTokens,
	};
	use crate::{
		AccountBindingId, ClientId, Command, CommandOutcome, Core,
		CredentialSource, EventActor, EventKind, ModelConsumption, ModelId,
		PlaneUsage, ProviderId, Query, QueryResult, RetentionPolicy, Run,
	};

	/// A fixed instant, so a window's freshness follows the test's own clock
	/// rather than the machine's.
	const NOW: Duration = Duration::from_millis(1_700_000_000_000);

	/// A Run that selected no Account binding, which is what a Run started
	/// without one looks like to a Usage report.
	const UNBOUND: Option<AccountBindingId> = None;

	async fn start(dir: &tempfile::TempDir) -> (Core, Arc<ManualClock>) {
		let clock = ManualClock::at(UNIX_EPOCH + NOW);
		let core = start_core_with(
			&dir.path().join("plane.sqlite3"),
			clock.clone(),
			FixedProbe::new(equipped()),
		)
		.await;
		(core, clock)
	}

	/// One Conversation with one Run, which is everything a Usage record needs
	/// to name what it belongs to.
	async fn run(core: &Core) -> Run {
		let conversation_id = Uuid::now_v7();
		let run_id = Uuid::now_v7();
		let now = core.now_unix_ms();
		core.store
			.write(async |tx| {
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: now,
				})
				.await?;
				tx.insert_run(NewRun {
					run_id,
					conversation_id,
					created_at_unix_ms: now,
				})
				.await
			})
			.await
			.unwrap()
			.into()
	}

	/// One Account binding, which is what a Provider-reported window belongs
	/// to.
	async fn bind(core: &Core) -> AccountBindingId {
		let CommandOutcome::AccountBound(binding) = core
			.execute(
				&actor(),
				request(Command::BindAccount {
					provider: ProviderId("anthropic".into()),
					label: "Work".into(),
					provider_account: None,
					credential_source: CredentialSource::HarnessNative,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Account binding")
		};
		binding.binding_id
	}

	async fn record(
		core: &Core,
		run: &Run,
		binding: Option<AccountBindingId>,
		report: UsageReport,
	) {
		let actor = EventActor::Harness {
			run_id: run.run_id,
			authorized_by: ClientId(Uuid::nil()),
		};
		let now = core.now_unix_ms();
		core.store
			.write(async |tx| {
				crate::usage::record::record(
					tx, &actor, run, binding, report, now,
				)
				.await
			})
			.await
			.unwrap();
	}

	async fn usage(core: &Core, selection: UsageSelection) -> PlaneUsage {
		let QueryResult::Usage(usage) = core
			.query(&actor(), Query::Usage { selection })
			.await
			.unwrap()
		else {
			panic!("Usage")
		};
		*usage
	}

	/// Every Usage Event the journal holds, which is how a client learns there
	/// is something new to read.
	async fn recorded(core: &Core) -> Vec<EventKind> {
		events(core)
			.await
			.into_iter()
			.filter(|event| matches!(event, EventKind::UsageRecorded { .. }))
			.collect()
	}

	fn observed(turn: &str, tokens: UsageTokens) -> UsageReport {
		UsageReport::Observed(ObservedUsage {
			measurement: UsageMeasurement::Turn {
				turn: turn.into(),
				native_usage_id: None,
			},
			model: Some(ModelId("claude-opus-5".into())),
			estimation: UsageEstimation::Measured,
			finality: UsageFinality::Final,
			tokens,
		})
	}

	fn window(used: u64) -> UsageReport {
		UsageReport::ProviderQuota(reported("five_hour", used, 3_600))
	}

	fn window_named(name: &str, used: u64) -> UsageReport {
		UsageReport::ProviderQuota(reported(name, used, 3_600))
	}

	fn window_resetting_in(used: u64, seconds: u64) -> UsageReport {
		UsageReport::ProviderQuota(reported("five_hour", used, seconds))
	}

	/// One Provider-reported window, as a Craft states it.
	fn reported(
		window: &str,
		used: u64,
		resets_in_seconds: u64,
	) -> QuotaReport {
		QuotaReport {
			window: window.into(),
			scope: QuotaScope::ProviderAccount,
			measure: QuotaMeasure {
				unit: QuotaUnit::Share,
				used,
				limit: None,
			},
			window_seconds: Some(18_000),
			resets_in_seconds: Some(resets_in_seconds),
			estimation: UsageEstimation::Measured,
			finality: UsageFinality::Interim,
		}
	}

	fn tokens(input: u64, output: u64) -> UsageTokens {
		UsageTokens {
			input,
			cached_input: 0,
			output,
			reasoning: 0,
		}
	}

	/// Consumption is summed only after deduplication, and the Event journal
	/// carries one record per stored measurement (ADR-0023).
	#[tokio::test]
	async fn observed_consumption_is_recorded_once_per_measurement() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		record(&core, &run, UNBOUND, observed("turn-1", tokens(10, 5))).await;
		record(&core, &run, UNBOUND, observed("turn-2", tokens(4, 1))).await;
		// The same turn reported again replaces what it already covers.
		record(&core, &run, UNBOUND, observed("turn-2", tokens(6, 2))).await;
		let usage = usage(&core, UsageSelection::Run(run.run_id)).await;
		assert_eq!(
			(
				usage.consumption.tokens,
				usage.consumption.models,
				recorded(&core).await.len()
			),
			(
				tokens(16, 7),
				vec![ModelConsumption {
					model: Some(ModelId("claude-opus-5".into())),
					tokens: tokens(16, 7),
					measurements: 2,
					estimated: 0,
					interim: 0,
					last_observed_at: UNIX_EPOCH + NOW,
				}],
				3
			)
		);
	}

	/// A Provider that repeats itself is a heartbeat, not another snapshot,
	/// and a Provider that changed is a new one (ADR-0045).
	#[tokio::test]
	async fn an_unchanged_provider_response_does_not_become_another_snapshot() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(60));
		record(&core, &run, Some(binding), window(2_500)).await;
		let unchanged = recorded(&core).await.len();
		clock.advance(Duration::from_secs(60));
		record(&core, &run, Some(binding), window(4_000)).await;
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		let window = usage.quota_windows.first().expect("one window");
		assert_eq!(
			(
				unchanged,
				recorded(&core).await.len(),
				usage.quota_windows.len(),
				window.measure,
				window.freshness.clone()
			),
			(
				1,
				2,
				1,
				QuotaMeasure {
					unit: QuotaUnit::Share,
					used: 4_000,
					// A share always reports against its fixed limit, whatever
					// the Craft sent.
					limit: Some(10_000),
				},
				UsageFreshness::Fresh
			)
		);
	}

	/// Past the interval a Plane refreshes on, a window is history rather than
	/// a current reading (ADR-0045).
	#[tokio::test]
	async fn a_window_the_provider_has_not_confirmed_recently_is_stale() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(16 * 60));
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		assert_eq!(
			usage
				.quota_windows
				.into_iter()
				.map(|window| window.freshness)
				.collect::<Vec<_>>(),
			vec![UsageFreshness::Stale]
		);
	}

	/// An unchanged answer is still an answer. A Provider that keeps saying
	/// the same thing leaves heartbeats rather than snapshots, and the window
	/// stays fresh because freshness follows the last answer rather than the
	/// last change (ADR-0045).
	#[tokio::test]
	async fn a_repeated_answer_keeps_its_window_fresh() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(14 * 60));
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(2 * 60));
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		let window = usage.quota_windows.first().expect("one window");
		assert_eq!(
			(usage.quota_windows.len(), window.freshness.clone()),
			(1, UsageFreshness::Fresh)
		);
	}

	/// A window is confirmed by an answer about that window. A Provider that
	/// repeats one of its limits has said nothing about the others, and they
	/// go stale on their own (ADR-0023).
	#[tokio::test]
	async fn one_window_answering_does_not_refresh_another() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		record(&core, &run, Some(binding), window_named("weekly", 4_000)).await;
		// The Provider repeats its five-hour limit alone. The repeat is a
		// heartbeat rather than a snapshot, and it confirms that window only.
		clock.advance(Duration::from_secs(10 * 60));
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(8 * 60));
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		assert_eq!(
			usage
				.quota_windows
				.into_iter()
				.map(|window| (window.window, window.freshness))
				.collect::<Vec<_>>(),
			vec![
				("five_hour".to_owned(), UsageFreshness::Fresh),
				("weekly".to_owned(), UsageFreshness::Stale)
			]
		);
	}

	/// A Provider may state a window length of zero, which the Craft protocol
	/// allows and which no clock can mean. It is recorded as the absent length
	/// it is: refusing it would fail the whole source batch the report arrived
	/// in and take its Run down with it.
	#[tokio::test]
	async fn a_window_of_no_stated_length_is_recorded_without_one() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(
			&core,
			&run,
			Some(binding),
			UsageReport::ProviderQuota(QuotaReport {
				window_seconds: Some(0),
				..reported("five_hour", 2_500, 3_600)
			}),
		)
		.await;
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		assert_eq!(
			usage
				.quota_windows
				.into_iter()
				.map(|window| (window.window, window.window_seconds))
				.collect::<Vec<_>>(),
			vec![("five_hour".to_owned(), None)]
		);
	}

	/// A window whose own reset has passed describes a window that has
	/// already rolled over, whatever the Provider last said about it.
	#[tokio::test]
	async fn a_window_past_its_reset_is_stale() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window_resetting_in(2_500, 60))
			.await;
		// The Provider keeps answering, so the answer is recent. The window it
		// answered about is not: its own minute is over.
		clock.advance(Duration::from_secs(120));
		record(&core, &run, Some(binding), window_resetting_in(2_500, 60))
			.await;
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		assert_eq!(
			usage
				.quota_windows
				.into_iter()
				.map(|window| window.freshness)
				.collect::<Vec<_>>(),
			vec![UsageFreshness::Stale]
		);
	}

	/// A Provider that would not answer is reported as unreachable rather than
	/// as the window it last reported (ADR-0023).
	#[tokio::test]
	async fn a_provider_that_would_not_answer_is_reported_as_unreachable() {
		let dir = tempfile::tempdir().unwrap();
		let (core, clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		clock.advance(Duration::from_secs(60));
		record(
			&core,
			&run,
			Some(binding),
			UsageReport::ProviderUnreachable {
				reason: "the Harness reported no quota window".into(),
			},
		)
		.await;
		let usage = usage(&core, UsageSelection::Binding(binding)).await;
		let window = usage.quota_windows.first().expect("one window");
		assert_eq!(
			(window.measure.used, window.freshness.clone()),
			(
				2_500,
				UsageFreshness::Unreachable {
					reason: "the Harness reported no quota window".into()
				}
			)
		);
	}

	/// A quota window belongs to an Account binding. A Run that named none
	/// leaves the window unrecorded rather than attaching it to a guess.
	#[tokio::test]
	async fn a_quota_window_without_an_account_binding_is_not_recorded() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		record(&core, &run, UNBOUND, window(2_500)).await;
		let usage = usage(&core, UsageSelection::Plane).await;
		assert_eq!(
			(usage.quota_windows, recorded(&core).await),
			(vec![], vec![])
		);
	}

	/// A Conversation answers with what it consumed and leaves its account's
	/// windows to the account they belong to.
	#[tokio::test]
	async fn a_conversation_answers_with_consumption_alone() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		record(
			&core,
			&run,
			Some(binding),
			observed("turn-1", tokens(10, 5)),
		)
		.await;
		let usage =
			usage(&core, UsageSelection::Conversation(run.conversation_id))
				.await;
		assert_eq!(
			(usage.quota_windows, usage.consumption.tokens),
			(vec![], tokens(10, 5))
		);
	}

	/// The Event says what was recorded, so a client knows to read the Query
	/// again without the numbers being repeated into the journal.
	#[tokio::test]
	async fn the_journal_names_what_was_recorded_and_where_it_belongs() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		let binding = bind(&core).await;
		record(&core, &run, Some(binding), window(2_500)).await;
		record(&core, &run, UNBOUND, observed("turn-1", tokens(10, 5))).await;
		assert_eq!(
			recorded(&core).await,
			vec![
				EventKind::UsageRecorded {
					source: UsageSource::ProviderQuota,
					binding_id: Some(binding),
				},
				EventKind::UsageRecorded {
					source: UsageSource::JetObserved,
					binding_id: None,
				}
			]
		);
	}

	/// Bounded metadata is what a record carries; a Craft cannot make one out
	/// of a payload.
	#[tokio::test]
	async fn a_report_that_is_not_bounded_metadata_is_refused() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let run = run(&core).await;
		let actor = EventActor::Harness {
			run_id: run.run_id,
			authorized_by: ClientId(Uuid::nil()),
		};
		let now = core.now_unix_ms();
		let refused = core
			.store
			.write(async |tx| {
				crate::usage::record::record(
					tx,
					&actor,
					&run,
					UNBOUND,
					observed(&"t".repeat(129), tokens(1, 1)),
					now,
				)
				.await
			})
			.await
			.expect_err("bounded metadata");
		assert_eq!(refused.code, "usage.turn_unsupported");
	}

	/// A Plane answers for itself. Its snapshot names the Plane it came from
	/// so a client never reads it as an account-wide total (ADR-0016).
	#[tokio::test]
	async fn a_snapshot_names_the_plane_it_covers() {
		let dir = tempfile::tempdir().unwrap();
		let (core, _clock) = start(&dir).await;
		let plane_id = core
			.store
			.read(async |tx| tx.plane().await)
			.await
			.unwrap()
			.plane_id;
		let usage = usage(&core, UsageSelection::Plane).await;
		assert_eq!(usage.plane_id.0, plane_id);
	}
}
