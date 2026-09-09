//! Both bundled Crafts, exercised through their public one-shot extension protocol.
use jet_protocol::{
	CraftExtensionReply, CraftExtensionRequest, ExtensionAction,
	ExtensionConfirmation, ExtensionScope, ExtensionTrust,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};

async fn invoke(
	craft: &str,
	root: &Path,
	request: CraftExtensionRequest,
) -> CraftExtensionReply {
	let executable = std::env::current_exe()
		.unwrap()
		.parent()
		.unwrap()
		.parent()
		.unwrap()
		.join(craft);
	let mut child = Command::new(executable)
		.arg("--extensions-v1")
		.env(
			"PATH",
			format!(
				"{}:{}",
				root.display(),
				std::env::var("PATH").unwrap_or_default()
			),
		)
		.env("EXTENSION_TEST_ROOT", root)
		.env("HOME", root)
		.env("CODEX_HOME", root.join(".codex"))
		.env("CLAUDE_CONFIG_DIR", root.join(".claude"))
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let mut input = child.stdin.take().unwrap();
	input
		.write_all(&serde_json::to_vec(&request).unwrap())
		.await
		.unwrap();
	drop(input);
	let output = tokio::time::timeout(
		std::time::Duration::from_secs(10),
		child.wait_with_output(),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	serde_json::from_slice(&output.stdout).unwrap()
}
fn peer(root: &Path, name: &str) {
	let path = root.join(name);
	std::fs::write(&path, r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['EXTENSION_TEST_ROOT'])
metadata = json.loads((root / 'catalog.json').read_text())
if pathlib.Path(sys.argv[0]).name == 'claude':
    if sys.argv[1:] == ['plugin', 'list', '--available', '--json']:
        print(json.dumps(metadata))
    elif sys.argv[1:3] == ['plugin', 'details']:
        print('Source: official; Publisher: native publisher; Version: 1; Skills: inspect; Hooks: SessionStart; MCP: docs')
    else:
        with (root / 'calls').open('a') as f: f.write(json.dumps(sys.argv[1:]) + '\n')
else:
    assert sys.argv[1:] == ['app-server', '--stdio']
    for line in sys.stdin:
        req = json.loads(line)
        if req['method'] == 'initialized': continue
        if req['method'] == 'initialize': result = {}
        elif req['method'] == 'plugin/list': result = metadata
        elif req['method'] == 'plugin/read': result = {'plugin':{'summary':metadata['marketplaces'][0]['plugins'][0], 'skills':['inspect'], 'hooks':['SessionStart'], 'mcpServers':['docs']}}
        else:
            with (root / 'calls').open('a') as f: f.write(json.dumps({'method':req['method'], 'params':req['params']}) + '\n')
            result = {}
        print(json.dumps({'id':req['id'], 'result':result}), flush=True)
"#).unwrap();
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
		.unwrap();
}
#[tokio::test]
async fn bundled_crafts_preserve_native_metadata_and_route_all_lifecycle_operations()
 {
	for (craft, harness) in
		[("jet-craft-codex", "codex"), ("jet-craft-claude", "claude")]
	{
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path();
		peer(root, harness);
		let package = root.join("package");
		std::fs::create_dir(&package).unwrap();
		std::fs::write(
			package.join("SKILL.md"),
			"---\nname: inspect\n---\nInspect the project.",
		)
		.unwrap();
		if harness == "claude" {
			std::fs::create_dir_all(root.join(".claude/plugins")).unwrap();
			std::fs::create_dir(root.join(".claude-plugin")).unwrap();
			std::fs::write(root.join(".claude/plugins/known_marketplaces.json"),json!({"official":{"source":{"source":"directory","path":root},"installLocation":root}}).to_string()).unwrap();
			std::fs::write(root.join(".claude-plugin/marketplace.json"),json!({"name":"official","owner":{"name":"native publisher"},"plugins":[{"name":"tool","source":"./package","version":"1"}]}).to_string()).unwrap();
		}
		let plugin = json!({"id":"tool@official","name":"tool","version":"1","publisher":"native publisher","source":{"type":"local","path":package},"installPath":package});
		let metadata = if harness == "claude" {
			json!({"installed":[plugin],"available":[]})
		} else {
			json!({"marketplaces":[{"name":"official","path":"/native/catalog.json","plugins":[plugin]}]})
		};
		std::fs::write(root.join("catalog.json"), metadata.to_string())
			.unwrap();
		let CraftExtensionReply::Catalog { catalog } =
			invoke(craft, root, CraftExtensionRequest::Catalog).await
		else {
			panic!()
		};
		assert_eq!(serde_json::from_str::<Value>(&catalog.native_metadata).unwrap()["plugins"].clone(), metadata);
		let CraftExtensionReply::Catalog { catalog } = invoke(
			craft,
			root,
			CraftExtensionRequest::Inspect {
				extension_id: "tool@official".into(),
			},
		)
		.await
		else {
			panic!()
		};
		let preview: Value =
			serde_json::from_str(&catalog.native_metadata).unwrap();
		assert_eq!(preview["files"][0]["path"], "SKILL.md");
		for action in [
			ExtensionAction::Install,
			ExtensionAction::Update,
			ExtensionAction::Disable,
			ExtensionAction::Remove,
		] {
			let confirmation = ExtensionConfirmation {
				catalog: catalog.clone(),
				extension_id: "tool@official".into(),
				action,
				scope: ExtensionScope::User,
				trust: ExtensionTrust::SameUserExecutable,
			};
			assert!(matches!(
				invoke(
					craft,
					root,
					CraftExtensionRequest::Apply { confirmation }
				)
				.await,
				CraftExtensionReply::Applied
			));
		}
		let calls: Vec<Value> = std::fs::read_to_string(root.join("calls"))
			.unwrap()
			.lines()
			.map(|line| serde_json::from_str(line).unwrap())
			.collect();
		let expected = if harness == "claude" {
			json!([
				["plugin", "install", "tool@official", "--scope", "user"],
				["plugin", "update", "tool@official", "--scope", "user"],
				["plugin", "disable", "tool@official", "--scope", "user"],
				["plugin", "uninstall", "tool@official", "--scope", "user"]
			])
		} else {
			json!([
				{"method":"plugin/install","params":{"pluginName":"tool","marketplacePath":"/native/catalog.json"}}, {"method":"plugin/install","params":{"pluginName":"tool","marketplacePath":"/native/catalog.json"}}, {"method":"config/value/write","params":{"keyPath":"plugins.\"tool@official\".enabled","value":false,"mergeStrategy":"replace"}}, {"method":"plugin/uninstall","params":{"pluginId":"tool@official"}}
			])
		};
		assert_eq!(json!(calls), expected);
		std::fs::write(
			package.join("SKILL.md"),
			"Changed executable instructions",
		)
		.unwrap();
		let confirmation = ExtensionConfirmation {
			catalog: catalog.clone(),
			extension_id: "tool@official".into(),
			action: ExtensionAction::Update,
			scope: ExtensionScope::User,
			trust: ExtensionTrust::SameUserExecutable,
		};
		assert!(matches!(
			invoke(craft, root, CraftExtensionRequest::Apply { confirmation })
				.await,
			CraftExtensionReply::Refused
		));
		std::fs::write(root.join("catalog.json"), "{}").unwrap();
		let confirmation = ExtensionConfirmation {
			catalog,
			extension_id: "tool@official".into(),
			action: ExtensionAction::Install,
			scope: ExtensionScope::User,
			trust: ExtensionTrust::SameUserExecutable,
		};
		assert!(matches!(
			invoke(craft, root, CraftExtensionRequest::Apply { confirmation })
				.await,
			CraftExtensionReply::Refused
		));
		assert_eq!(
			std::fs::read_to_string(root.join("calls"))
				.unwrap()
				.lines()
				.count(),
			4
		);
	}
}

#[tokio::test]
async fn standalone_extensions_preserve_native_files_and_unrelated_settings() {
	for (craft, harness) in
		[("jet-craft-codex", "codex"), ("jet-craft-claude", "claude")]
	{
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path();
		peer(root, harness);
		std::fs::write(root.join("catalog.json"), "{}").unwrap();
		let source = root.join("source");
		std::fs::create_dir(&source).unwrap();
		std::fs::write(
			source.join("SKILL.md"),
			"---\nname: sample\n---\nInspect sources",
		)
		.unwrap();
		let native = root.join(if harness == "codex" {
			".codex"
		} else {
			".claude"
		});
		std::fs::create_dir(&native).unwrap();
		let mcp = if harness == "codex" {
			native.join("config.toml")
		} else {
			native.join(".claude.json")
		};
		let mcp_source = root.join(if harness == "codex" {
			"source.toml"
		} else {
			"source.json"
		});
		let hook = native.join(if harness == "codex" {
			"hooks.json"
		} else {
			"settings.json"
		});
		let hook_source = root.join("hooks-source.json");
		if harness == "codex" {
			std::fs::write(&mcp,"# retained comment\nmodel = \"retained\"\n[mcp_servers.other]\ncommand = \"keep\"\n").unwrap();
			std::fs::write(&mcp_source,"[mcp_servers.docs]\ncommand = \"server\"\n[mcp_servers.docs.env]\nAPI_TOKEN = \"private-value\"\n").unwrap();
		} else {
			std::fs::write(
				&mcp,
				r#"{"theme":"retained","mcpServers":{"other":{"command":"keep"}}}"#,
			)
			.unwrap();
			std::fs::write(&mcp_source,r#"{"mcpServers":{"docs":{"command":"server","env":{"API_TOKEN":"private-value"}}}}"#).unwrap();
		}
		std::fs::write(&hook,r#"{"unrelated":"retained","hooks":{"Stop":[{"hooks":[{"type":"command","command":"keep"}]}]}}"#).unwrap();
		std::fs::write(&hook_source,r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"startup"}]}]}}"#).unwrap();
		for (kind, name, path) in [
			("skill", "sample", source),
			("mcp", "docs", mcp_source),
			("hook", "SessionStart", hook_source),
		] {
			let source_id = format!("{kind}:{name}@{}", path.display());
			let installed_id = format!("{kind}:{name}");
			for (id, action) in [
				(&source_id, ExtensionAction::Install),
				(&source_id, ExtensionAction::Update),
				(&installed_id, ExtensionAction::Disable),
				(&installed_id, ExtensionAction::Install),
				(&installed_id, ExtensionAction::Remove),
			] {
				let CraftExtensionReply::Catalog { catalog } = invoke(
					craft,
					root,
					CraftExtensionRequest::Inspect {
						extension_id: id.clone(),
					},
				)
				.await
				else {
					panic!()
				};
				assert!(!catalog.native_metadata.contains("private-value"));
				let confirmation = ExtensionConfirmation {
					catalog,
					extension_id: id.clone(),
					action,
					scope: ExtensionScope::User,
					trust: ExtensionTrust::SameUserExecutable,
				};
				assert!(matches!(
					invoke(
						craft,
						root,
						CraftExtensionRequest::Apply { confirmation }
					)
					.await,
					CraftExtensionReply::Applied
				));
			}
		}
		let mcp_text = std::fs::read_to_string(mcp).unwrap();
		assert!(
			mcp_text.contains("retained")
				&& mcp_text.contains("keep")
				&& !mcp_text.contains("private-value")
		);
		if harness == "codex" {
			assert!(mcp_text.contains("# retained comment"));
		}
		assert_eq!(
			serde_json::from_str::<Value>(
				&std::fs::read_to_string(hook).unwrap()
			)
			.unwrap(),
			json!({"unrelated":"retained","hooks":{"Stop":[{"hooks":[{"type":"command","command":"keep"}]}]}})
		);
	}
}

#[tokio::test]
async fn invalid_standalone_extensions_are_refused_before_native_settings_change()
 {
	for (craft, harness) in
		[("jet-craft-codex", "codex"), ("jet-craft-claude", "claude")]
	{
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path();
		let source = root.join("source");
		let malformed = if harness == "codex" {
			"[mcp_servers.docs]\ncommand = 123\n"
		} else {
			r#"{"mcpServers":{"docs":{"command":123}}}"#
		};
		let invalid_transport = if harness == "codex" {
			"[mcp_servers.docs]\ncommand = \"\"\nurl = \"https://example.test\"\n"
		} else {
			r#"{"mcpServers":{"docs":{"command":"","url":"https://example.test"}}}"#
		};
		let invalid_timeout = if harness == "codex" {
			"[mcp_servers.docs]\ncommand = \"server\"\nstartup_timeout_ms = 1.5\n"
		} else {
			r#"{"mcpServers":{"docs":{"command":"server","startup_timeout_ms":1.5}}}"#
		};
		let wrong_headers = if harness == "codex" {
			"[mcp_servers.docs]\ncommand = \"server\"\n[mcp_servers.docs.http_headers]\nx = \"y\"\n"
		} else {
			r#"{"mcpServers":{"docs":{"command":"server","headers":{"x":"y"}}}}"#
		};
		let wrong_args = if harness == "codex" {
			"[mcp_servers.docs]\nurl = \"https://example.test\"\nargs = [\"x\"]\n"
		} else {
			r#"{"mcpServers":{"docs":{"url":"https://example.test","args":["x"]}}}"#
		};
		for input in [
			malformed,
			invalid_transport,
			invalid_timeout,
			wrong_headers,
			wrong_args,
		] {
			std::fs::write(&source, input).unwrap();
			assert!(matches!(
				invoke(
					craft,
					root,
					CraftExtensionRequest::Inspect {
						extension_id: format!("mcp:docs@{}", source.display())
					}
				)
				.await,
				CraftExtensionReply::Refused
			));
		}
		let native = root.join(if harness == "codex" {
			".codex/config.toml"
		} else {
			".claude/.claude.json"
		});
		assert!(!native.exists());
	}
}
