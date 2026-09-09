//! Native Codex app-server plugin catalog and lifecycle, without a Conversation.
use jet_craft_sdk::extension_error;
use jet_protocol::{
	CraftExtensionReply, CraftExtensionRequest, ExtensionAction,
	ExtensionCatalog,
};
use serde_json::{Value, json};
use std::{io, process::Stdio};
use tokio::{
	io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
	process::{Child, ChildStdin, ChildStdout, Command},
};

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
				craft_id: "codex".into(),
				harness: "codex".into(),
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
	let native = std::env::var_os("CODEX_HOME")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| home.join(".codex"));
	let skills = home.join(".agents/skills");
	let configurations = vec![
		NativeConfig {
			kind: "mcp",
			path: native.join("config.toml"),
			key: "mcp_servers",
			format: NativeConfigFormat::Toml,
		},
		NativeConfig {
			kind: "hook",
			path: native.join("hooks.json"),
			key: "hooks",
			format: NativeConfigFormat::Json,
		},
		NativeConfig {
			kind: "hook-config",
			path: native.join("config.toml"),
			key: "hooks",
			format: NativeConfigFormat::Toml,
		},
	];
	Ok(StandaloneExtensions {
		harness: "codex",
		skills,
		configurations,
	})
}

async fn plugins(
	request: CraftExtensionRequest,
) -> io::Result<CraftExtensionReply> {
	let mut native = Native::start().await?;
	let metadata = native.call("plugin/list", json!({})).await?;
	let catalog = ExtensionCatalog {
		craft_id: "codex".into(),
		harness: "codex".into(),
		native_metadata: metadata.to_string(),
	};
	let reply = match request {
		CraftExtensionRequest::Catalog => {
			CraftExtensionReply::Catalog { catalog }
		}
		CraftExtensionRequest::Inspect { extension_id } => {
			let catalog =
				inspect(&mut native, &metadata, &extension_id).await?;
			CraftExtensionReply::Catalog { catalog }
		}
		CraftExtensionRequest::Apply { confirmation } => {
			if selected(&metadata, &confirmation.extension_id).is_none() {
				return Ok(CraftExtensionReply::Refused);
			}
			let catalog =
				inspect(&mut native, &metadata, &confirmation.extension_id)
					.await?;
			if confirmation.catalog != catalog
				|| !jet_craft_sdk::extension_review_complete(
					&catalog.native_metadata,
				) {
				return Ok(CraftExtensionReply::Refused);
			}
			let Some((marketplace, plugin)) =
				selected(&metadata, &confirmation.extension_id)
			else {
				return Ok(CraftExtensionReply::Refused);
			};
			match confirmation.action {
				ExtensionAction::Install | ExtensionAction::Update => {
					let mut params = json!({"pluginName": plugin["name"]});
					if let Some(path) = marketplace["path"].as_str() {
						params["marketplacePath"] = json!(path);
					} else {
						params["remoteMarketplaceName"] =
							marketplace["name"].clone();
					}
					native.call("plugin/install", params).await?;
				}
				ExtensionAction::Disable => {
					// JSON quoting keeps the plugin identity one TOML key segment.
					let key = format!(
						"plugins.{}.enabled",
						serde_json::to_string(&confirmation.extension_id)?
					);
					native
						.call(
							"config/value/write",
							json!({"keyPath":key,"value":false,"mergeStrategy":"replace"}),
						)
						.await?;
				}
				ExtensionAction::Remove => {
					native
						.call(
							"plugin/uninstall",
							json!({"pluginId":confirmation.extension_id}),
						)
						.await?;
				}
			}
			CraftExtensionReply::Applied
		}
	};
	native.child.kill().await?;
	Ok(reply)
}
async fn inspect(
	native: &mut Native,
	metadata: &Value,
	id: &str,
) -> io::Result<ExtensionCatalog> {
	let (marketplace, plugin) =
		selected(metadata, id).ok_or_else(extension_error)?;
	let mut params = json!({"pluginName":plugin["name"]});
	if let Some(path) = marketplace["path"].as_str() {
		params["marketplacePath"] = json!(path);
	} else {
		params["remoteMarketplaceName"] = marketplace["name"].clone();
	}
	let detail = native.call("plugin/read", params).await?;
	let root = detail
		.pointer("/plugin/summary/source/path")
		.and_then(Value::as_str);
	let files = match root {
		Some(root) if std::path::Path::new(root).is_dir() => {
			let root = std::path::PathBuf::from(root);
			serde_json::to_value(
				tokio::task::spawn_blocking(move || {
					jet_craft_sdk::extension_files(&root)
				})
				.await
				.map_err(|_| extension_error())??,
			)?
		}
		_ => Value::Null,
	};
	Ok(ExtensionCatalog { craft_id: "codex".into(), harness: "codex".into(), native_metadata: json!({"extension_id":id,"summary":plugin,"details":detail,"files":files,"permissions":jet_craft_sdk::extension_host_access()}).to_string() })
}
fn selected<'a>(
	metadata: &'a Value,
	id: &str,
) -> Option<(&'a Value, &'a Value)> {
	for marketplace in metadata["marketplaces"].as_array()? {
		for plugin in marketplace["plugins"].as_array()? {
			if plugin["id"].as_str() == Some(id) {
				return Some((marketplace, plugin));
			}
		}
	}
	None
}
struct Native {
	child: Child,
	input: ChildStdin,
	output: BufReader<ChildStdout>,
	id: u32,
}
impl Native {
	async fn start() -> io::Result<Self> {
		let mut child = Command::new("codex")
			.args(["app-server", "--stdio"])
			.current_dir("/")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::null())
			.kill_on_drop(true)
			.spawn()?;
		let input = child.stdin.take().ok_or_else(extension_error)?;
		let output =
			BufReader::new(child.stdout.take().ok_or_else(extension_error)?);
		let mut native = Self {
			child,
			input,
			output,
			id: 0,
		};
		native.call("initialize", json!({"clientInfo":{"name":"jet-extensions","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
		native
			.input
			.write_all(b"{\"method\":\"initialized\"}\n")
			.await?;
		Ok(native)
	}
	async fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
		self.id += 1;
		let request = json!({"id":self.id,"method":method,"params":params});
		self.input
			.write_all(format!("{request}\n").as_bytes())
			.await?;
		// ASVS 2.2.1: bound both a single record and notification noise.
		for _ in 0..64 {
			let mut line = Vec::new();
			(&mut self.output)
				.take(65537)
				.read_until(b'\n', &mut line)
				.await?;
			if line.is_empty()
				|| line.len() > 65536
				|| line.last() != Some(&b'\n')
			{
				return Err(extension_error());
			}
			let value: Value = jet_protocol::decode_control(&line)
				.map_err(|_| extension_error())?;
			if value["id"] == self.id {
				return value
					.get("result")
					.cloned()
					.ok_or_else(extension_error);
			}
		}
		Err(extension_error())
	}
}
