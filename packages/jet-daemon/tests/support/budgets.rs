//! Measurement support for the ADR-0022 performance gates: seeding a Plane
//! at the reference scale, timing one operation many times, and writing
//! the measurements `.github/scripts/budgets.py` gates against the accepted
//! baseline.
use jet_store::{
	ActorRecord, ConversationOriginRecord, ConversationPageStart, EventClass,
	NewConversation, NewEvent, RetentionPolicy, Store, WorkingTreeRecord,
};
use serde::Serialize;
use std::{
	path::{Path, PathBuf},
	time::{Duration, Instant},
};
use uuid::Uuid;

/// How much of the reference Plane to build. The reference scale is what
/// ADR-0022 budgets; the smoke scale keeps the runner exercised in the
/// ordinary suite.
#[derive(Debug, Clone, Copy)]
pub enum Scale {
	Reference,
	Smoke,
}

impl Scale {
	pub fn conversations(self) -> usize {
		match self {
			Self::Reference => 10_000,
			Self::Smoke => 100,
		}
	}

	pub fn events(self) -> usize {
		match self {
			Self::Reference => 1_000_000,
			Self::Smoke => 2_000,
		}
	}

	/// How many times each timed operation runs.
	pub fn samples(self) -> usize {
		match self {
			Self::Reference => 200,
			Self::Smoke => 5,
		}
	}
}

/// One run of every gate, in the units `budgets.toml` names.
#[derive(Debug, Serialize)]
pub struct Measurements {
	pub os: &'static str,
	pub arch: &'static str,
	pub conversations: usize,
	pub events: usize,
	pub store_open_ms: f64,
	pub daemon_ready_ms: f64,
	pub daemon_ready_again_ms: f64,
	pub commit_64_events_p99_ms: f64,
	pub commit_256_kib_p99_ms: f64,
	pub sidebar_page_p95_ms: f64,
	pub blocks_500_p95_ms: f64,
	pub reconnect_10000_events_ms: f64,
	pub ingestion_events_per_second: f64,
}

/// Where a run's measurements go for `.github/scripts/budgets.py`: one file per
/// operating system and architecture under the workspace target directory.
pub fn output_path() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../target/budgets")
		.join(format!(
			"{}-{}.json",
			std::env::consts::OS,
			std::env::consts::ARCH
		))
}

pub fn write(measurements: &Measurements, path: &Path) {
	std::fs::create_dir_all(path.parent().unwrap()).unwrap();
	std::fs::write(path, serde_json::to_vec_pretty(measurements).unwrap())
		.unwrap();
}

fn event(
	conversation_id: Option<Uuid>,
	kind: &str,
	payload: String,
) -> NewEvent {
	NewEvent {
		event_id: Uuid::now_v7(),
		actor: ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		},
		recorded_at_unix_ms: 0,
		conversation_id,
		run_id: None,
		kind: kind.to_owned(),
		payload_version: 1,
		payload,
		class: match conversation_id {
			Some(_) => EventClass::Semantic,
			None => EventClass::Operational,
		},
	}
}

fn small_payload() -> String {
	"{\"activity\":\"running\",\"progress\":\"one small step\"}".to_owned()
}

/// Seeds `scale` Conversations and journal Events, one of which holds
/// 500 transcript Events for the 500-block query, and returns that
/// Conversation.
pub async fn seed(store: &Store, scale: Scale) -> Uuid {
	let transcript = Uuid::now_v7();
	let mut remaining = scale.events();
	store
		.write(async |tx| {
			for index in 0..scale.conversations() {
				tx.insert_conversation(NewConversation {
					conversation_id: if index == 0 {
						transcript
					} else {
						Uuid::now_v7()
					},
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::NoProject,
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: 0,
				})
				.await?;
			}
			for _ in 0..500 {
				tx.append_event(event(
					Some(transcript),
					"run.output",
					small_payload(),
				))
				.await?;
			}
			Ok::<_, jet_store::StoreError>(())
		})
		.await
		.unwrap();
	while remaining > 0 {
		let batch = remaining.min(10_000);
		store
			.write(async |tx| {
				for _ in 0..batch {
					tx.append_event(event(
						None,
						"run.progress",
						small_payload(),
					))
					.await?;
				}
				Ok::<_, jet_store::StoreError>(())
			})
			.await
			.unwrap();
		remaining -= batch;
	}
	transcript
}

fn millis(duration: Duration) -> f64 {
	duration.as_secs_f64() * 1_000.0
}

/// The `percentile` of `samples`, by nearest rank.
fn percentile(mut samples: Vec<Duration>, percentile: usize) -> f64 {
	samples.sort_unstable();
	let rank = (samples.len() * percentile).div_ceil(100).max(1);
	millis(samples[rank - 1])
}

async fn timed_samples<F>(count: usize, mut operation: F) -> Vec<Duration>
where
	F: AsyncFnMut(),
{
	let mut samples = Vec::with_capacity(count);
	for _ in 0..count {
		let started = Instant::now();
		operation().await;
		samples.push(started.elapsed());
	}
	samples
}

/// The shape of one representative commit ADR-0022 budgets: how many
/// Events it carries and how large each payload is.
#[derive(Debug, Clone, Copy)]
pub struct Commit {
	pub events: usize,
	pub payload_bytes: usize,
}

impl Commit {
	/// Sixty-four small Events, the batched ingestion shape.
	pub const SMALL_BATCH: Self = Self {
		events: 64,
		payload_bytes: 0,
	};
	/// 256 KiB across eight Events, under the 64 KiB payload bound.
	pub const LARGE: Self = Self {
		events: 8,
		payload_bytes: 32 * 1024,
	};
}

pub async fn commit_events(store: &Store, commit: Commit) {
	store
		.write(async |tx| {
			for _ in 0..commit.events {
				tx.append_event(event(
					None,
					"run.progress",
					payload(commit.payload_bytes),
				))
				.await?;
			}
			Ok::<_, jet_store::StoreError>(())
		})
		.await
		.unwrap();
}

fn payload(bytes: usize) -> String {
	format!("{{\"blob\":\"{}\"}}", "x".repeat(bytes.saturating_sub(11)))
}

/// p99 of one `commit` shape, `samples` times.
pub async fn commit_p99(store: &Store, samples: usize, commit: Commit) -> f64 {
	percentile(
		timed_samples(samples, async || {
			commit_events(store, commit).await;
		})
		.await,
		99,
	)
}

/// p95 of the first sidebar page.
pub async fn sidebar_p95(store: &Store, samples: usize) -> f64 {
	percentile(
		timed_samples(samples, async || {
			store
				.read(async |tx| {
					tx.conversation_page(ConversationPageStart::First).await
				})
				.await
				.unwrap();
		})
		.await,
		95,
	)
}

/// p95 of reading one Conversation's 500 transcript Events.
pub async fn blocks_p95(
	store: &Store,
	conversation: Uuid,
	samples: usize,
) -> f64 {
	percentile(
		timed_samples(samples, async || {
			let events = store
				.read(async |tx| tx.transcript_events(conversation).await)
				.await
				.unwrap();
			assert_eq!(events.len(), 500);
		})
		.await,
		95,
	)
}

/// Median time to page 10,000 Events after a cursor the way a
/// reconnecting client does, in the store's bounded pages.
pub async fn reconnect_ms(store: &Store, samples: usize, events: usize) -> f64 {
	let head = store
		.read(async |tx| tx.event_cursor().await)
		.await
		.unwrap();
	let start = head.saturating_sub(u64::try_from(events).unwrap());
	percentile(
		timed_samples(samples, async || {
			let mut cursor = start;
			let mut read = 0;
			while read < events {
				let page = store
					.read(async |tx| {
						tx.events_after(cursor, events - read).await
					})
					.await
					.unwrap()
					.1;
				assert!(!page.is_empty(), "the journal ran out at {cursor}");
				cursor = page.last().unwrap().sequence;
				read += page.len();
			}
		})
		.await,
		50,
	)
}

/// Small Events per second when ingested in commits of 64.
pub async fn ingestion_rate(store: &Store, events: usize) -> f64 {
	let started = Instant::now();
	let mut remaining = events;
	while remaining > 0 {
		let batch = remaining.min(Commit::SMALL_BATCH.events);
		commit_events(
			store,
			Commit {
				events: batch,
				..Commit::SMALL_BATCH
			},
		)
		.await;
		remaining -= batch;
	}
	events as f64 / started.elapsed().as_secs_f64()
}

pub fn elapsed_ms(started: Instant) -> f64 {
	millis(started.elapsed())
}
