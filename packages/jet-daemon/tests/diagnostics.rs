//! A real `jetd` keeps a bounded, owner-only, redacted Diagnostic log
//! under its Jet home (ADR-0061).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{ErrorCategory, ServerHello};
use pretty_assertions::assert_eq;
use std::os::unix::fs::PermissionsExt;
use support::{handshake_raw, hello, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn the_daemon_records_its_life_and_refusals_without_content() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let mut daemon = start_jetd(&home).await;

	// A hello nobody can honor is refused, and the refusal is recorded by
	// its code alone.
	let mut unsupported = hello(Uuid::new_v4());
	unsupported.codec = "codec/nobody-speaks".into();
	let (_raw, rejected) = handshake_raw(&daemon, &unsupported).await;
	let ServerHello::Rejected { error } = rejected else {
		panic!("expected a rejected hello, got {rejected:?}");
	};
	assert_eq!(error.category, ErrorCategory::Incompatible);

	let pid = rustix::process::Pid::from_raw(
		i32::try_from(daemon.child.id().unwrap()).unwrap(),
	)
	.unwrap();
	rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
	assert!(daemon.child.wait().await.unwrap().success());

	let log_dir = home.join("diagnostics");
	let log = log_dir.join("jetd.log");
	assert_eq!(
		std::fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777,
		0o700
	);
	assert_eq!(
		std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
		0o600
	);
	let records: Vec<serde_json::Value> = std::fs::read_to_string(&log)
		.unwrap()
		.lines()
		.map(|line| {
			let mut record: serde_json::Value =
				serde_json::from_str(line).unwrap();
			record.as_object_mut().unwrap().remove("time");
			record
		})
		.collect();
	let summary: Vec<(String, String, String)> = records
		.iter()
		.map(|record| {
			(
				record["level"].as_str().unwrap().to_owned(),
				record["component"].as_str().unwrap().to_owned(),
				record["message"].as_str().unwrap().to_owned(),
			)
		})
		.collect();
	let started = (
		"info".to_owned(),
		"process".to_owned(),
		"daemon started".to_owned(),
	);
	let rejected = (
		"warn".to_owned(),
		"connection".to_owned(),
		"rejected client hello".to_owned(),
	);
	let stopping = (
		"info".to_owned(),
		"process".to_owned(),
		"daemon stopping".to_owned(),
	);
	assert!(summary.contains(&started), "{summary:?}");
	assert!(summary.contains(&rejected), "{summary:?}");
	assert_eq!(summary.last(), Some(&stopping), "{summary:?}");
	let refusal = records
		.iter()
		.find(|record| record["message"] == "rejected client hello")
		.unwrap();
	assert_eq!(
		refusal["fields"],
		serde_json::json!({"code": "protocol.unsupported_codec"})
	);
	// The codec the client named never reaches the log.
	assert!(
		!std::fs::read_to_string(&log)
			.unwrap()
			.contains("nobody-speaks")
	);
}
