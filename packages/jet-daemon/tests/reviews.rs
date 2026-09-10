//! The real bundled reviewer entrypoints against a fake external transport
//! (ADR-0012). What matters here is what a reviewer is given and what it is
//! allowed to answer with: one request, no tools, the exact action, and a
//! judgement the trusted core still has to accept.
use jet_protocol::{CraftReviewModel, CraftReviewReply, CraftReviewer};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};

mod support;

#[tokio::test]
async fn approval_retry_commands_require_the_new_minor_and_a_live_denial() {
	let dir = tempfile::tempdir().unwrap();
	let daemon = support::start_jetd(&dir.path().join(".jet")).await;
	let command = json!({"kind":"command", "id":1, "command_id":uuid::Uuid::now_v7(),
		"command":{"type":"authorize_approval_retry", "run_id":uuid::Uuid::new_v4(), "review_id":uuid::Uuid::new_v4()}});
	for (minor, code) in [
		(jet_protocol::ARTIFACTS_MINOR, "protocol.unsupported_minor"),
		(jet_protocol::APPROVAL_RETRY_MINOR, "review.run_unavailable"),
	] {
		let mut hello = support::hello(uuid::Uuid::new_v4());
		hello.minor = minor;
		let (mut wire, _) = support::handshake_raw(&daemon, &hello).await;
		wire.send(&command).await;
		assert_eq!(wire.receive::<Value>().await["error"]["code"], code);
	}
}

fn executable(path: &Path, source: &str) {
	std::fs::write(path, source).unwrap();
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
		.unwrap();
}

async fn invoke(
	craft: &Path,
	root: &Path,
	mode: &str,
	input: &Value,
) -> std::process::Output {
	let mut child = Command::new(craft)
		.arg(mode)
		.env(
			"PATH",
			format!(
				"{}:{}",
				root.display(),
				std::env::var("PATH").unwrap_or_default()
			),
		)
		.env("UTILITY_TEST_ROOT", root)
		.env_remove("OPENAI_API_KEY")
		.env_remove("ANTHROPIC_API_KEY")
		.current_dir(root)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let mut stdin = child.stdin.take().unwrap();
	if mode == "--review" {
		stdin
			.write_all(&serde_json::to_vec(input).unwrap())
			.await
			.unwrap();
	}
	drop(stdin);
	tokio::time::timeout(
		std::time::Duration::from_secs(10),
		child.wait_with_output(),
	)
	.await
	.unwrap()
	.unwrap()
}

#[tokio::test]
async fn bundled_reviewers_send_one_tool_free_request_carrying_only_the_transcript_and_action()
 {
	for (craft_name, provider) in [
		("jet-craft-codex", "openai"),
		("jet-craft-claude", "anthropic"),
	] {
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path();
		let craft = std::env::current_exe()
			.unwrap()
			.parent()
			.unwrap()
			.parent()
			.unwrap()
			.join(craft_name);
		executable(
			&root.join("key-helper"),
			"#!/bin/sh\n[ \"$1\" = jet-utility-api-key ] || exit 1\nprintf '%s' 'TEST-KEY-NEVER-PROMPT-CONTENT'\n",
		);
		executable(
			&root.join("curl"),
			r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['UTILITY_TEST_ROOT'])
assert sys.argv[1] == '--disable'
assert '--location' not in sys.argv and '--retry' not in sys.argv
config = {}
for line in sys.stdin.read().splitlines():
    key, value = line.split(' = ', 1)
    config.setdefault(key, []).append(json.loads(value))
with (root / 'calls').open('a') as file: file.write('one\n')
(root / 'capture').write_text(json.dumps(config))
sys.stdout.write((root / 'response').read_text())
"#,
		);
		let selected =
			invoke(&craft, root, "--review-model", &Value::Null).await;
		assert!(selected.status.success());
		let selected: CraftReviewModel =
			serde_json::from_slice(&selected.stdout).unwrap();
		assert_eq!(
			(selected.version, selected.reviewer),
			(1, CraftReviewer::Equivalent)
		);
		let model = selected.model;
		let request = json!({"version":1,"model":model,"binding_id":"00000000-0000-0000-0000-000000000039",
            "credential_reference":{"source":"external_helper","helper":root.join("key-helper")},
            "input":{"transcript":"user: run the tests\nharness: about to run them","tool":"Bash","action":"{\"command\":\"cargo test\"}"}});
		let answer = "{\"risk\":\"low\",\"authorization\":\"sufficient\",\"decision\":\"allow\",\"rationale\":\"The user asked for the tests.\"}";
		let response = if provider == "openai" {
			json!({"model":model,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}]})
		} else {
			json!({"model":model,"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":answer}]})
		};
		std::fs::write(root.join("response"), response.to_string()).unwrap();
		let output = invoke(&craft, root, "--review", &request).await;
		assert!(output.status.success(), "{output:?}");
		let reply: CraftReviewReply =
			serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(
			reply,
			CraftReviewReply {
				version: 1,
				model: model.clone(),
				reviewer: CraftReviewer::Equivalent,
				output: answer.into(),
			}
		);
		assert_eq!(
			std::fs::read_to_string(root.join("calls")).unwrap(),
			"one\n"
		);
		let capture: Value = serde_json::from_slice(
			&std::fs::read(root.join("capture")).unwrap(),
		)
		.unwrap();
		let body: Value =
			serde_json::from_str(capture["data-binary"][0].as_str().unwrap())
				.unwrap();
		// A reviewer gets no tools, and the request carries the transcript
		// and the exact action without ever carrying the Credential.
		assert_eq!(
			(&body["model"], &body["tools"]),
			(&json!(model), &json!([]))
		);
		assert!(body.to_string().contains("cargo test"));
		assert!(!body.to_string().contains("TEST-KEY"));
		assert!(!body.to_string().contains("credential_reference"));
		assert_eq!(
			capture["url"],
			json!([if provider == "openai" {
				"https://api.openai.com/v1/responses"
			} else {
				"https://api.anthropic.com/v1/messages"
			}])
		);
		// A Provider reporting a substituted Model is refused, never retried.
		let mut wrong = response;
		wrong["model"] = json!("different-model");
		std::fs::write(root.join("response"), wrong.to_string()).unwrap();
		let output = invoke(&craft, root, "--review", &request).await;
		assert!(!output.status.success());
		assert!(output.stdout.is_empty() && output.stderr.is_empty());
		// An oversized transcript never reaches the credential or the
		// transport: the bound is checked before either is touched.
		let mut oversized = request;
		oversized["input"]["transcript"] = json!("x".repeat(12 * 1024 + 1));
		assert!(
			!invoke(&craft, root, "--review", &oversized)
				.await
				.status
				.success()
		);
		assert_eq!(
			std::fs::read_to_string(root.join("calls")).unwrap(),
			"one\none\n"
		);
	}
}
