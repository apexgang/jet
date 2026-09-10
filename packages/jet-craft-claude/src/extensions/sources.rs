//! Inspect local incoming marketplace content separately from installed cache files.
use jet_craft_sdk::{extension_error, extension_files};
use serde_json::{Value, json};
use std::{
	io::{self, Read},
	path::{Path, PathBuf},
};

pub(super) fn inspect(plugin: &Value, id: &str) -> io::Result<Value> {
	let installed = plugin["installPath"].as_str().map(PathBuf::from);
	let installed_files = installed
		.as_ref()
		.map(|root| extension_files(root))
		.transpose()?
		.map(serde_json::to_value)
		.transpose()?
		.unwrap_or(Value::Null);
	let candidate = incoming(id)?;
	let files = candidate
		.as_ref()
		.map(|value| value["files"].clone())
		.unwrap_or_else(|| installed_files.clone());
	Ok(
		json!({"installed_files":installed_files,"candidate":candidate,"files":files}),
	)
}
fn incoming(id: &str) -> io::Result<Option<Value>> {
	let (name, marketplace) = id.split_once('@').ok_or_else(extension_error)?;
	let config = std::env::var_os("CLAUDE_CONFIG_DIR")
		.map(PathBuf::from)
		.or_else(|| {
			std::env::var_os("HOME")
				.map(|home| PathBuf::from(home).join(".claude"))
		})
		.ok_or_else(extension_error)?;
	let registry = config.join("plugins/known_marketplaces.json");
	if !registry.exists() {
		return Ok(None);
	}
	let known = read(&registry)?;
	let source = &known[marketplace]["source"];
	// Native install refreshes Git/URL marketplaces. Their cached files cannot
	// establish consent to an incoming artifact, even with auto-update disabled.
	let Some(kind @ ("directory" | "file")) = source["source"].as_str() else {
		return Ok(None);
	};
	let Some(path) = source["path"]
		.as_str()
		.map(PathBuf::from)
		.filter(|p| p.is_absolute())
	else {
		return Ok(None);
	};
	let manifest = if kind == "directory" {
		path.join(".claude-plugin/marketplace.json")
	} else {
		path
	};
	let root = manifest.parent().ok_or_else(extension_error)?;
	let root = if root
		.file_name()
		.is_some_and(|name| name == ".claude-plugin")
	{
		root.parent().ok_or_else(extension_error)?
	} else {
		root
	};
	let marketplace = read(&manifest)?;
	let Some(entry) = marketplace["plugins"].as_array().and_then(|plugins| {
		plugins.iter().find(|p| p["name"].as_str() == Some(name))
	}) else {
		return Ok(None);
	};
	let Some(relative) = entry["source"].as_str() else {
		return Ok(None);
	};
	let mut relative = PathBuf::from(relative);
	if relative.components().count() == 1
		&& !relative.to_string_lossy().starts_with('.')
		&& let Some(prefix) = marketplace
			.pointer("/metadata/pluginRoot")
			.and_then(Value::as_str)
	{
		relative = Path::new(prefix).join(relative);
	}
	if relative.is_absolute()
		|| relative
			.components()
			.any(|c| matches!(c, std::path::Component::ParentDir))
	{
		return Ok(None);
	}
	let package = root.join(relative);
	let files = extension_files(&package)?;
	let plugin_manifest = package.join(".claude-plugin/plugin.json");
	let plugin = if plugin_manifest.exists() {
		read(&plugin_manifest)?
	} else {
		json!({})
	};
	if entry.get("dependencies").is_some()
		|| plugin.get("dependencies").is_some()
		|| package.join("package.json").exists()
	{
		return Ok(None);
	}
	// Bind native marketplace overrides and its registry declaration as well as files.
	let registry_files = jet_craft_sdk::extension_file_identity(&registry)?;
	let marketplace_file = jet_craft_sdk::extension_file_identity(&manifest)?;
	Ok(Some(
		json!({"source":source,"path":package,"version":entry.get("version").unwrap_or(&plugin["version"]),"publisher":entry.get("author").unwrap_or(&plugin["author"]),"component_keys":entry.as_object().map(|o|o.keys().collect::<Vec<_>>()),"files":files,"marketplace":marketplace_file,"registry":registry_files}),
	))
}
fn read(path: &Path) -> io::Result<Value> {
	if path.symlink_metadata()?.file_type().is_symlink() {
		return Err(extension_error());
	}
	let file = std::fs::File::open(path)?;
	if !file.metadata()?.is_file() {
		return Err(extension_error());
	}
	let mut bytes = Vec::new();
	file.take(65537).read_to_end(&mut bytes)?;
	if bytes.len() > 65536 {
		return Err(extension_error());
	}
	jet_protocol::decode_control(&bytes).map_err(|_| extension_error())
}
