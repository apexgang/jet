//! Real bundled Utility entrypoints against a fake external inference transport.
use jet_protocol::{CraftUtilityModel, CraftUtilityReply};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};

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
	if mode == "--utility" {
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
async fn bundled_utilities_send_one_tool_free_request_with_exact_authentication_and_model()
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
			invoke(&craft, root, "--utility-model", &Value::Null).await;
		assert!(selected.status.success());
		let selected: CraftUtilityModel =
			serde_json::from_slice(&selected.stdout).unwrap();
		let model = selected.model;
		let request = json!({"version":1,"model":model,"binding_id":"00000000-0000-0000-0000-000000000038",
            "credential_reference":{"source":"external_helper","helper":root.join("key-helper")},
            "input":{"purpose":"autodelete","prompt":"Inactive for 90 days"}});
		let answer = "{\"inactive_days\":90}";
		let response = if provider == "openai" {
			json!({"model":model,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}]})
		} else {
			json!({"model":model,"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":answer}]})
		};
		std::fs::write(root.join("response"), response.to_string()).unwrap();
		let output = invoke(&craft, root, "--utility", &request).await;
		assert!(output.status.success(), "{:?}", output);
		let reply: CraftUtilityReply =
			serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(
			(reply.version, reply.model, reply.output),
			(1, model.clone(), answer.into())
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
		assert_eq!(
			(&body["model"], &body["tools"]),
			(&json!(model), &json!([]))
		);
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
		assert!(capture["header"].as_array().unwrap().iter().any(|h| {
			h.as_str()
				.unwrap()
				.contains("TEST-KEY-NEVER-PROMPT-CONTENT")
		}));
		if provider == "openai" {
			assert_eq!(body["reasoning"], json!({"effort":"none"}));
			assert_eq!(body["store"], false);
		} else {
			assert_eq!(body["thinking"], json!({"type":"disabled"}));
		}
		// A Provider reporting a substituted Model is refused, never retried.
		let mut wrong = response;
		wrong["model"] = json!("different-model");
		std::fs::write(root.join("response"), wrong.to_string()).unwrap();
		let output = invoke(&craft, root, "--utility", &request).await;
		assert!(!output.status.success());
		assert!(output.stdout.is_empty() && output.stderr.is_empty());
		assert_eq!(
			std::fs::read_to_string(root.join("calls")).unwrap(),
			"one\none\n"
		);
		// Oversized input never invokes the credential or inference transport.
		let mut oversized = request;
		oversized["input"]["prompt"] = json!("x".repeat(4097));
		assert!(
			!invoke(&craft, root, "--utility", &oversized)
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

mod support;
#[tokio::test]
async fn daemon_dispatches_an_accepted_craft_and_retains_the_attributed_result()
{
	use jet_protocol::{
		CommandRequest, CommandResponse, CredentialSource, SettingKey,
		SettingScope, SettingValue, UtilityOutcome, UtilityRequest,
	};
	use sha2::{Digest, Sha256};
	use uuid::Uuid;
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let root = dir.path();
        let home = root.join("jet");
        std::fs::create_dir_all(home.join("crafts")).unwrap();
        let craft = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("jet-craft-codex");
        let selected = invoke(&craft, root, "--utility-model", &Value::Null).await;
        let model: CraftUtilityModel = serde_json::from_slice(&selected.stdout).unwrap();
        let specification = jet_craft_sdk::parse_specification(include_str!("../../jet-craft-codex/.jet/craft-spec.toml")).unwrap();
        let installation = json!({"executable":craft,"sha256":format!("{:x}",Sha256::digest(std::fs::read(&craft).unwrap())),"specification":specification});
        std::fs::write(home.join("crafts/codex.json"), installation.to_string()).unwrap();
        let response = json!({"model":model.model,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"inactive_days\":90}"}]}]});
        std::fs::write(root.join("response"), response.to_string()).unwrap();
        executable(&root.join("key-helper"), "#!/bin/sh\nprintf '%s' 'TEST-KEY'\n");
        executable(&root.join("curl"), "#!/usr/bin/env python3\nimport os,pathlib,sys\nroot=pathlib.Path(os.environ['UTILITY_TEST_ROOT'])\nsys.stdin.read()\nwith (root/'calls').open('a') as f: f.write('one\\n')\nsys.stdout.write((root/'response').read_text())\n");
        let mut process = support::jetd(&home);
        process.env("PATH",format!("{}:{}",root.display(),std::env::var("PATH").unwrap_or_default())).env("UTILITY_TEST_ROOT",root);
        let mut daemon = support::start_jetd_process(&mut process).await;
        let owner = Uuid::new_v4();
        let client = support::connect(&daemon, owner).await;
        let binding = client.bind_account(Uuid::now_v7(), "openai", "Utility", None, CredentialSource::ExternalHelper { helper: root.join("key-helper").to_str().unwrap().into() }).await.unwrap();
        client.set_setting(Uuid::now_v7(), SettingKey::UtilityAccountBinding, SettingScope::Plane, SettingValue::Text(binding.binding_id.to_string())).await.unwrap();
        client.set_setting(Uuid::now_v7(), SettingKey::UtilityAutodeleteCompilation, SettingScope::Plane, SettingValue::Flag(true)).await.unwrap();
        let command_id = Uuid::now_v7();
        let command = CommandRequest::RequestUtility { request: UtilityRequest::Autodelete { prompt: "Inactive for 90 days".into() } };
        let admitted = client.execute_command(command_id, command.clone()).await.unwrap();
        let CommandResponse::UtilityQueued { job_id } = admitted else { panic!("Utility") };
        let job = loop {
            let job = client.utility(job_id).await.unwrap();
            if job.outcome != UtilityOutcome::Pending { break job; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!((&job.provider, job.binding_id, &job.model, &job.outcome), (&Some("openai".into()), Some(binding.binding_id), &Some(model.model), &UtilityOutcome::Draft { inactive_days: 90 }));
        daemon.child.kill().await.unwrap();
        let daemon = support::start_jetd(&home).await;
        let client = support::connect(&daemon, owner).await;
        assert_eq!(client.execute_command(command_id, command).await.unwrap(), admitted);
        assert_eq!(client.utility(job_id).await.unwrap(), job);
        assert_eq!(std::fs::read_to_string(root.join("calls")).unwrap(), "one\n");
    }).await.unwrap();
}
