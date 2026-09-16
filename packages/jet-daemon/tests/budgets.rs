//! The ADR-0022 store, startup, reconnect, and ingestion measurements.
//! `just budget-test` runs them alone on a reference host at the
//! reference scale and gates the result through `.github/scripts/budgets.py`;
//! the ordinary suite runs the same code at a smoke scale so the runner
//! cannot rot between reference runs.
#[path = "support/budgets.rs"]
mod measured;
mod support;
use measured::{Measurements, Scale};
use pretty_assertions::assert_eq;
use std::{path::Path, time::Instant};

/// Builds a Plane at `scale`, measures every gate, and writes the result
/// to `output`.
async fn measure(scale: Scale, root: &Path, output: &Path) -> Measurements {
	let home = root.join("jet");
	jet_runtime::JetHome::at(home.clone()).prepare().unwrap();
	let database = home.join("plane.sqlite3");
	let store = jet_store::Store::open(&database).await.unwrap();
	let transcript = measured::seed(&store, scale).await;
	store.close().await;

	let started = Instant::now();
	let store = jet_store::Store::open(&database).await.unwrap();
	let store_open_ms = measured::elapsed_ms(started);
	let samples = scale.samples();
	let commit_64_events_p99_ms =
		measured::commit_p99(&store, samples, measured::Commit::SMALL_BATCH)
			.await;
	let commit_256_kib_p99_ms =
		measured::commit_p99(&store, samples, measured::Commit::LARGE).await;
	let sidebar_page_p95_ms = measured::sidebar_p95(&store, samples).await;
	let blocks_500_p95_ms =
		measured::blocks_p95(&store, transcript, samples).await;
	let reconnect_10000_events_ms = measured::reconnect_ms(
		&store,
		samples.min(20),
		scale.events().min(10_000),
	)
	.await;
	let ingestion_events_per_second =
		measured::ingestion_rate(&store, scale.events().min(100_000)).await;
	store.close().await;

	let started = Instant::now();
	let mut daemon = support::start_jetd(&home).await;
	let daemon_ready_ms = measured::elapsed_ms(started);
	// kill_on_drop requests termination but does not wait for the Plane lock.
	daemon.child.kill().await.unwrap();
	// The first start of a day copies a Recovery snapshot (ADR-0097); the
	// second start on the same Plane shows the ready time without it.
	let started = Instant::now();
	let mut daemon = support::start_jetd(&home).await;
	let daemon_ready_again_ms = measured::elapsed_ms(started);
	daemon.child.kill().await.unwrap();

	let measurements = Measurements {
		os: std::env::consts::OS,
		arch: std::env::consts::ARCH,
		conversations: scale.conversations(),
		events: scale.events(),
		store_open_ms,
		daemon_ready_ms,
		daemon_ready_again_ms,
		commit_64_events_p99_ms,
		commit_256_kib_p99_ms,
		sidebar_page_p95_ms,
		blocks_500_p95_ms,
		reconnect_10000_events_ms,
		ingestion_events_per_second,
	};
	measured::write(&measurements, output);
	measurements
}

#[tokio::test]
#[ignore = "reference performance measurement (ADR-0022); just budget-test"]
async fn reference_store_and_startup_budgets() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let measurements =
		measure(Scale::Reference, dir.path(), &measured::output_path()).await;
	println!("{}", serde_json::to_string(&measurements).unwrap());
}

#[tokio::test]
async fn the_budget_runner_measures_every_gate_at_smoke_scale() {
	let dir = tempfile::tempdir().unwrap();
	let output = dir.path().join("budgets.json");
	let measurements = measure(Scale::Smoke, dir.path(), &output).await;
	let written: serde_json::Value =
		serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
	let not_positive: Vec<&str> = written
		.as_object()
		.unwrap()
		.iter()
		.filter(|(_, value)| value.as_f64().is_some_and(|value| value <= 0.0))
		.map(|(name, _)| name.as_str())
		.collect();
	assert_eq!(
		(
			measurements.conversations,
			measurements.events,
			not_positive
		),
		(100, 2_000, Vec::<&str>::new())
	);
}
