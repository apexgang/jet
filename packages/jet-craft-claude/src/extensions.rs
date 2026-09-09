//! Claude's native plugin lifecycle, including its skills, hooks and MCP components.
#[path = "extension_sources.rs"]
mod sources;
use jet_craft_sdk::extension_error;
use jet_protocol::{
	CraftExtensionReply, CraftExtensionRequest, ExtensionAction,
	ExtensionCatalog,
};
use serde_json::{Value, json};
use std::{io, process::Stdio};
use tokio::{io::AsyncReadExt, process::Command};

pub(crate) async fn handle(
	request: CraftExtensionRequest,
) -> io::Result<CraftExtensionReply> {
	let standalone = standalone()?;
	let id = match &request {
		CraftExtensionRequest::Inspect { extension_id } => {
			Some(extension_id.as_str())
		}
		CraftExtensionRequest::Apply { confirmation } => {
			Some(confirmation.extension_id.as_str())
		}
		CraftExtensionRequest::Catalog => None,
	};
	if id.is_some_and(|id| standalone.accepts(id)) {
		return tokio::task::spawn_blocking(move || standalone.handle(request))
			.await
			.map_err(|_| extension_error())?;
	}
	if matches!(&request, CraftExtensionRequest::Catalog) {
		let inventory =
			tokio::task::spawn_blocking(move || standalone.catalog())
				.await
				.map_err(|_| extension_error())??;
		let mut catalog = match plugins(request).await {
			Ok(CraftExtensionReply::Catalog { catalog }) => catalog,
			Ok(CraftExtensionReply::Applied | CraftExtensionReply::Refused)
			| Err(_) => ExtensionCatalog {
				craft_id: "claude-code".into(),
				harness: "claude-code".into(),
				native_metadata: "{\"plugin_catalog_unavailable\":true}".into(),
			},
		};
		let metadata: Value = serde_json::from_str(&catalog.native_metadata)?;
		catalog.native_metadata =
			json!({"plugins":metadata,"standalone":inventory}).to_string();
		return Ok(CraftExtensionReply::Catalog { catalog });
	}
	plugins(request).await
}
fn standalone() -> io::Result<jet_craft_sdk::StandaloneExtensions> {
	use jet_craft_sdk::{
		NativeConfig, NativeConfigFormat, StandaloneExtensions,
	};
	let home = std::env::var_os("HOME")
		.map(std::path::PathBuf::from)
		.filter(|p| p.is_absolute())
		.ok_or_else(extension_error)?;
	let native = std::env::var_os("CLAUDE_CONFIG_DIR")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| home.join(".claude"));
	let skills = native.join("skills");
	let configurations = vec![
		NativeConfig {
			kind: "mcp",
			path: std::env::var_os("CLAUDE_CONFIG_DIR")
				.map(|p| std::path::PathBuf::from(p).join(".claude.json"))
				.unwrap_or_else(|| home.join(".claude.json")),
			key: "mcpServers",
			format: NativeConfigFormat::Json,
		},
		NativeConfig {
			kind: "hook",
			path: native.join("settings.json"),
			key: "hooks",
			format: NativeConfigFormat::Json,
		},
	];
	Ok(StandaloneExtensions {
		harness: "claude-code",
		skills,
		configurations,
	})
}

async fn plugins(
	request: CraftExtensionRequest,
) -> io::Result<CraftExtensionReply> {
	let catalog = catalog().await?;
	match request {
		CraftExtensionRequest::Catalog => {
			Ok(CraftExtensionReply::Catalog { catalog })
		}
		CraftExtensionRequest::Inspect { extension_id } => {
			Ok(CraftExtensionReply::Catalog {
				catalog: inspect(&catalog, &extension_id).await?,
			})
		}
		CraftExtensionRequest::Apply { confirmation } => {
			let Some(_) = selected(
				&serde_json::from_str(&catalog.native_metadata)?,
				&confirmation.extension_id,
			) else {
				return Ok(CraftExtensionReply::Refused);
			};
			let catalog = inspect(&catalog, &confirmation.extension_id).await?;
			if confirmation.catalog != catalog
				|| !jet_craft_sdk::extension_review_complete(
					&catalog.native_metadata,
				) || !selector(&confirmation.extension_id)
			{
				return Ok(CraftExtensionReply::Refused);
			}
			let preview: Value =
				serde_json::from_str(&catalog.native_metadata)?;
			let reviewed = if matches!(
				confirmation.action,
				ExtensionAction::Install | ExtensionAction::Update
			) {
				!preview["candidate"].is_null()
			} else {
				preview["installed_files"]
					.as_array()
					.is_some_and(|files| !files.is_empty())
			};
			if !reviewed {
				return Ok(CraftExtensionReply::Refused);
			}
			let operation = match confirmation.action {
				ExtensionAction::Install => "install",
				ExtensionAction::Update => "update",
				ExtensionAction::Disable => "disable",
				ExtensionAction::Remove => "uninstall",
			};
			// ASVS 1.2.5: a closed command and one validated selector; never invoke a shell.
			native(&[
				"plugin",
				operation,
				&confirmation.extension_id,
				"--scope",
				"user",
			])
			.await?;
			Ok(CraftExtensionReply::Applied)
		}
	}
}
async fn catalog() -> io::Result<ExtensionCatalog> {
	let bytes = native(&["plugin", "list", "--available", "--json"]).await?;
	let metadata: serde_json::Value =
		jet_protocol::decode_control(&bytes).map_err(|_| extension_error())?;
	Ok(ExtensionCatalog {
		craft_id: "claude-code".into(),
		harness: "claude-code".into(),
		native_metadata: metadata.to_string(),
	})
}
async fn inspect(
	catalog: &ExtensionCatalog,
	id: &str,
) -> io::Result<ExtensionCatalog> {
	if !selector(id) {
		return Err(extension_error());
	}
	let metadata: Value = serde_json::from_str(&catalog.native_metadata)?;
	let plugin = selected(&metadata, id).ok_or_else(extension_error)?;
	let plugin = plugin.clone();
	let target = id.to_owned();
	let mut preview =
		tokio::task::spawn_blocking(move || sources::inspect(&plugin, &target))
			.await
			.map_err(|_| extension_error())??;
	preview["extension_id"] = json!(id);
	preview["summary"] =
		selected(&metadata, id).cloned().unwrap_or(Value::Null);
	preview["permissions"] = jet_craft_sdk::extension_host_access();
	preview["supported_actions"] = if preview["candidate"].is_null() {
		json!(["disable", "remove"])
	} else {
		json!(["install", "update", "disable", "remove"])
	};
	Ok(ExtensionCatalog {
		craft_id: catalog.craft_id.clone(),
		harness: catalog.harness.clone(),
		native_metadata: preview.to_string(),
	})
}
fn selected<'a>(metadata: &'a Value, id: &str) -> Option<&'a Value> {
	// Claude's JSON separates installed and available native entries.
	let groups = if let Some(entries) = metadata.as_array() {
		vec![entries]
	} else {
		["installed", "available"]
			.into_iter()
			.filter_map(|key| metadata[key].as_array())
			.collect()
	};
	for group in groups {
		for plugin in group {
			if plugin["scope"]
				.as_str()
				.is_some_and(|scope| scope != "user")
			{
				continue;
			}
			if plugin["id"].as_str() == Some(id) {
				return Some(plugin);
			}
			if let (Some(name), Some(marketplace)) =
				(plugin["name"].as_str(), plugin["marketplace"].as_str())
				&& id == format!("{name}@{marketplace}")
			{
				return Some(plugin);
			}
		}
	}
	None
}
fn selector(id: &str) -> bool {
	id.len() <= 160
		&& id.split_once('@').is_some_and(|(name, source)| {
			!name.is_empty() && !source.is_empty()
		}) && !id.starts_with('-')
		&& id
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}
async fn native(args: &[&str]) -> io::Result<Vec<u8>> {
	let mut child = Command::new("claude")
		.args(args)
		.current_dir("/")
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true)
		.spawn()?;
	let mut bytes = Vec::new();
	child
		.stdout
		.take()
		.ok_or_else(extension_error)?
		.take(65537)
		.read_to_end(&mut bytes)
		.await?;
	if bytes.len() > 65536 || !child.wait().await?.success() {
		return Err(extension_error());
	}
	Ok(bytes)
}
