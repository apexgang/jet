//! Native directory and configuration lifecycle for paths selected by a Craft.
//! Selectors address native files; this module defines no extension package format.
use crate::{
	extension_error, extension_files, extension_host_access,
	native_config::{self, NativeConfig},
};
use jet_protocol::{
	CraftExtensionReply, CraftExtensionRequest, ExtensionAction,
	ExtensionCatalog,
};
use serde_json::{Value, json};
use std::{
	io,
	path::{Path, PathBuf},
};
#[path = "standalone_changes.rs"]
mod changes;

/// Native user locations supplied by the Harness adapter, never by GUI code.
pub struct StandaloneExtensions {
	/// Installed Craft and Harness identity.
	pub harness: &'static str,
	/// Native skill discovery directory.
	pub skills: PathBuf,
	/// Native configuration collections supported by this Harness.
	pub configurations: Vec<NativeConfig>,
}
struct Target<'a> {
	kind: &'a str,
	name: &'a str,
	source: Option<PathBuf>,
}
fn target(id: &str) -> io::Result<Target<'_>> {
	let (kind, rest) = id.split_once(':').ok_or_else(extension_error)?;
	let (name, source) =
		rest.split_once('@').map_or((rest, None), |(name, path)| {
			(name, Some(PathBuf::from(path)))
		});
	if name.is_empty()
		|| name.len() > 100
		|| name.starts_with(['.', '-'])
		|| !name
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
		|| source.as_ref().is_some_and(|p| {
			!p.is_absolute()
				|| p.components()
					.any(|c| matches!(c, std::path::Component::ParentDir))
		}) {
		return Err(extension_error());
	}
	Ok(Target { kind, name, source })
}
impl StandaloneExtensions {
	/// Whether a selector belongs to the standalone native interface.
	pub fn accepts(&self, id: &str) -> bool {
		id.split_once(':').is_some_and(|(kind, _)| {
			kind == "skill"
				|| self.configurations.iter().any(|c| c.kind == kind)
		})
	}
	/// Discover configured native targets; explicit sources are inspected by selector.
	/// # Errors
	/// Refuses malformed or oversized native configuration.
	pub fn catalog(&self) -> io::Result<Value> {
		let mut entries = Vec::new();
		for (path, enabled) in
			[(&self.skills, true), (&self.disabled_skills(), false)]
		{
			if path.is_dir() {
				for entry in std::fs::read_dir(path)? {
					let entry = entry?;
					let Some(name) =
						entry.file_name().to_str().map(str::to_owned)
					else {
						continue;
					};
					if entry.file_type()?.is_dir()
						&& entry.path().join("SKILL.md").is_file()
					{
						entries.push(json!({"id":format!("skill:{name}"),"path":entry.path(),"enabled":enabled}));
					}
				}
			}
		}
		for config in &self.configurations {
			let value = config.value(&config.path)?;
			if let Some(collection) = value[config.key].as_object() {
				for name in collection.keys() {
					entries.push(json!({"id":format!("{}:{name}",config.kind),"path":config.path,"native_key":config.key,"enabled":true}));
				}
			}
			let disabled = self.disabled_config(config);
			let value = config.value(&disabled)?;
			if let Some(collection) = value[config.key].as_object() {
				for name in collection.keys() {
					entries.push(json!({"id":format!("{}:{name}",config.kind),"path":disabled,"native_key":config.key,"enabled":false}));
				}
			}
		}
		entries
			.sort_by_key(|e| e["id"].as_str().unwrap_or_default().to_owned());
		if entries.len() > 512 {
			return Err(extension_error());
		}
		Ok(
			json!({"entries":entries,"scope":"user","explicit_sources":"kind:name@/absolute/native/path (including Git checkouts)","permissions":extension_host_access()}),
		)
	}
	/// Inspect or mutate a standalone native target using the public Craft protocol.
	/// # Errors
	/// Refuses unsafe paths, unsupported native data, or changed review snapshots.
	pub fn handle(
		&self,
		request: CraftExtensionRequest,
	) -> io::Result<CraftExtensionReply> {
		match request {
			CraftExtensionRequest::Catalog => {
				Ok(CraftExtensionReply::Catalog {
					catalog: self.wrap(self.catalog()?),
				})
			}
			CraftExtensionRequest::Inspect { extension_id } => {
				Ok(match self.inspect(&extension_id) {
					Ok(catalog) => CraftExtensionReply::Catalog { catalog },
					Err(_) => CraftExtensionReply::Refused,
				})
			}
			CraftExtensionRequest::Apply { confirmation } => {
				let Ok(preview) = self.inspect(&confirmation.extension_id)
				else {
					return Ok(CraftExtensionReply::Refused);
				};
				if preview != confirmation.catalog {
					return Ok(CraftExtensionReply::Refused);
				}
				let selected = target(&confirmation.extension_id)?;
				if matches!(
					confirmation.action,
					ExtensionAction::Install | ExtensionAction::Update
				) && selected.source.is_none()
					&& !self.is_disabled(&selected)
				{
					return Ok(CraftExtensionReply::Refused);
				}
				let confirmed: Value = serde_json::from_str(
					&confirmation.catalog.native_metadata,
				)?;
				if selected.kind == "skill" {
					self.change_skill(
						&selected,
						confirmation.action,
						&confirmed,
					)?;
				} else {
					self.change_config(
						&selected,
						confirmation.action,
						&confirmed,
					)?;
				}
				Ok(CraftExtensionReply::Applied)
			}
		}
	}
	fn wrap(&self, metadata: Value) -> ExtensionCatalog {
		ExtensionCatalog {
			craft_id: self.harness.into(),
			harness: self.harness.into(),
			native_metadata: metadata.to_string(),
		}
	}
	fn disabled_skills(&self) -> PathBuf {
		self.skills
			.parent()
			.unwrap_or(&self.skills)
			.join(".jet-disabled-skills")
	}
	fn disabled_config(&self, config: &NativeConfig) -> PathBuf {
		config.path.with_file_name(format!(
			".jet-disabled-{}",
			config
				.path
				.file_name()
				.unwrap_or_default()
				.to_string_lossy()
		))
	}
	fn config(&self, kind: &str) -> io::Result<&NativeConfig> {
		self.configurations
			.iter()
			.find(|c| c.kind == kind)
			.ok_or_else(extension_error)
	}
	fn is_disabled(&self, selected: &Target<'_>) -> bool {
		if selected.kind == "skill" {
			self.disabled_skills().join(selected.name).is_dir()
		} else {
			self.config(selected.kind).ok().is_some_and(|c| {
				c.value(&self.disabled_config(c))
					.ok()
					.is_some_and(|v| !v[c.key][selected.name].is_null())
			})
		}
	}
	fn inspect(&self, id: &str) -> io::Result<ExtensionCatalog> {
		let selected = target(id)?;
		let (source, destination, files, native, current) = if selected.kind
			== "skill"
		{
			let destination = self.skills.join(selected.name);
			let default = if destination.exists() {
				destination.clone()
			} else {
				self.disabled_skills().join(selected.name)
			};
			let source = selected.source.clone().unwrap_or(default);
			if !source.join("SKILL.md").is_file() {
				return Err(extension_error());
			}
			let files = serde_json::to_value(extension_files(&source)?)?;
			let current = json!({"installed": if destination.exists() { serde_json::to_value(extension_files(&destination)?)? } else { Value::Null },"disabled":if self.disabled_skills().join(selected.name).exists() {serde_json::to_value(extension_files(&self.disabled_skills().join(selected.name))?)?} else {Value::Null}});
			(
				source,
				destination,
				files,
				json!({"skills":[selected.name]}),
				current,
			)
		} else {
			let config = self.config(selected.kind)?;
			let default = if config.value(&config.path)?[config.key]
				[selected.name]
				.is_null()
			{
				self.disabled_config(config)
			} else {
				config.path.clone()
			};
			let source = selected.source.clone().unwrap_or(default);
			let value = config.value(&source)?;
			let entry = value[config.key][selected.name].clone();
			if !crate::native_validation::valid(
				self.harness,
				selected.kind,
				selected.name,
				&entry,
			) {
				return Err(extension_error());
			}
			let files = json!([{"path":source,"sha256":native_config::digest(&native_config::read(&source)?)}]);
			(
				source,
				config.path.clone(),
				files,
				redact(entry),
				json!({"sha256":native_config::digest(&native_config::read(&config.path)?),"disabled_sha256":native_config::digest(&native_config::read(&self.disabled_config(config))?)}),
			)
		};
		Ok(self.wrap(json!({"extension_id":id,"source":source,"publisher":"user-selected native source","version":git_revision(&source),"files":files,"components":native,"destination":destination,"current":current,"disabled":self.is_disabled(&selected),"permissions":extension_host_access(),"scope":"user","trust":"requires_same_user_consent"})))
	}
}
fn redact(value: Value) -> Value {
	match value {
		Value::Object(values) => Value::Object(
			values
				.into_iter()
				.map(|(key, value)| {
					let lowered = key.to_ascii_lowercase();
					let value =
						if ["env", "headers"].contains(&lowered.as_str()) {
							value.as_object().map_or(
								json!("redacted"),
								|v| json!({"keys":v.keys().collect::<Vec<_>>()}),
							)
						} else if [
							"token",
							"secret",
							"password",
							"authorization",
							"apikey",
							"api_key",
						]
						.iter()
						.any(|part| lowered.contains(part))
						{
							json!("redacted")
						} else {
							redact(value)
						};
					(key, value)
				})
				.collect(),
		),
		Value::Array(values) => {
			Value::Array(values.into_iter().map(redact).collect())
		}
		Value::String(_) => {
			json!("native value omitted; inspect the source file")
		}
		other => other,
	}
}
fn git_revision(source: &Path) -> Value {
	let dir = if source.is_dir() {
		source
	} else {
		source.parent().unwrap_or(source)
	};
	// Read-only Git identity; never fetch, execute source commands, or evaluate a shell.
	std::process::Command::new("git")
		.args(["-C"])
		.arg(dir)
		.args(["rev-parse", "--verify", "HEAD"])
		.stdin(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.map_or(Value::Null, |v| json!(v.trim()))
}
