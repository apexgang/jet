//! Real process lifetime with a controlled clock at the RunHost boundary.
use super::*;
use jet_core::RunHost;
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn active_crafts_survive_and_idle_crafts_exit_after_five_minutes() {
	use std::os::unix::fs::PermissionsExt;
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let program = dir.path().join("craft");
	let binary = std::env::current_exe().unwrap();
	let quoted_binary = binary.to_str().unwrap().replace('\'', "'\\''");
	let script = format!(
		"#!/bin/sh\nJET_TEST_CRAFT_SOCKET=\"$2\" exec '{quoted_binary}' --ignored --exact craft_processes::tests::peer\n"
	);
	std::fs::write(&program, &script).unwrap();
	std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	let pin = PinnedCraft {
		id: "idle-test".into(),
		executable: program,
		sha256: format!("{:x}", Sha256::digest(script)),
		adapter_state: String::new(),
	};
	let host = CraftProcesses {
		supervisor: Some(
			binary.parent().unwrap().parent().unwrap().join("jetd"),
		),
		..CraftProcesses::default()
	};
	let mut stream = host.connect(dir.path(), &pin).await.unwrap();
	assert_eq!(stream.read_u8().await.unwrap(), 1);
	tokio::time::pause();
	for _ in 0..2 {
		host.maintain_crafts(vec![pin.clone()], vec![], vec![])
			.await
			.unwrap();
		tokio::time::advance(Duration::from_secs(301)).await;
	}
	assert!(
		tokio::time::timeout(Duration::from_millis(1), stream.read_u8())
			.await
			.is_err()
	);
	host.maintain_crafts(vec![], vec![], vec![]).await.unwrap();
	tokio::time::advance(Duration::from_secs(299)).await;
	host.maintain_crafts(vec![], vec![], vec![]).await.unwrap();
	assert!(
		tokio::time::timeout(Duration::from_millis(1), stream.read_u8())
			.await
			.is_err()
	);
	tokio::time::advance(Duration::from_secs(1)).await;
	tokio::time::resume();
	host.maintain_crafts(vec![], vec![], vec![]).await.unwrap();
	assert!(
		matches!(
			tokio::time::timeout(Duration::from_secs(1), stream.read(&mut [0]))
				.await,
			Ok(Ok(0))
		),
		"idle Craft must exit"
	);
}

#[test]
#[ignore = "invoked as a separate Craft process by the lifetime test"]
fn peer() {
	use std::io::{Read, Write};
	let listener = std::os::unix::net::UnixListener::bind(
		std::env::var_os("JET_TEST_CRAFT_SOCKET").unwrap(),
	)
	.unwrap();
	for stream in listener.incoming() {
		std::thread::spawn(move || {
			let mut stream = stream.unwrap();
			stream.write_all(&[1]).unwrap();
			let _ = stream.read_to_end(&mut Vec::new());
		});
	}
}
