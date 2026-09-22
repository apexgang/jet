//! Native package files are identified by streamed digests under one budget.
use jet_craft_sdk::extension_files;
use pretty_assertions::assert_eq;
use serde_json::json;
use sha2::{Digest, Sha256};

const BUDGET: usize = 16 * 1024 * 1024;

#[test]
fn files_are_identified_by_the_lowercase_sha256_of_their_content() {
	let root = tempfile::tempdir().unwrap();
	// Longer than one read so the digest spans several loop iterations.
	let content: Vec<u8> =
		(0..3 * 8192 + 17).map(|i| (i % 251) as u8).collect();
	std::fs::write(root.path().join("SKILL.md"), &content).unwrap();

	let files =
		serde_json::to_value(extension_files(root.path()).unwrap()).unwrap();

	assert_eq!(
		files,
		json!([{"path":"SKILL.md","sha256":hex::encode(Sha256::digest(&content))}])
	);
}

#[test]
fn the_aggregate_budget_counts_every_streamed_byte() {
	let root = tempfile::tempdir().unwrap();
	let large = vec![0u8; BUDGET];
	std::fs::write(root.path().join("large"), &large).unwrap();
	assert_eq!(
		serde_json::to_value(extension_files(root.path()).unwrap()).unwrap(),
		json!([{"path":"large","sha256":hex::encode(Sha256::digest(&large))}])
	);

	std::fs::write(root.path().join("one-more"), b"!").unwrap();
	let Err(error) = extension_files(root.path()) else {
		panic!("the extra byte exceeds the budget")
	};

	assert_eq!(error.kind(), std::io::ErrorKind::Other);
}
