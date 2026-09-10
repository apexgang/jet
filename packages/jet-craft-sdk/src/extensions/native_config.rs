//! Bounded edits to a selected native configuration key, preserving its siblings.
use crate::extension_error;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
	io::{self, Read, Write},
	path::{Path, PathBuf},
};

/// Native configuration syntax chosen by the responsible Harness adapter.
#[derive(Clone, Copy)]
pub enum NativeConfigFormat {
	/// Native JSON object.
	Json,
	/// Native TOML document; edits preserve comments and sibling keys.
	Toml,
}
/// One native collection, such as Claude's mcpServers or Codex's mcp_servers.
#[derive(Clone)]
pub struct NativeConfig {
	/// Selector prefix disclosed by the Craft.
	pub kind: &'static str,
	/// User configuration file.
	pub path: PathBuf,
	/// Native top-level collection key.
	pub key: &'static str,
	/// Native file syntax.
	pub format: NativeConfigFormat,
}
pub(crate) fn read(path: &Path) -> io::Result<Vec<u8>> {
	if !path.exists() {
		return Ok(Vec::new());
	}
	if path.symlink_metadata()?.file_type().is_symlink() {
		return Err(extension_error());
	}
	let file = file(path)?;
	if !file.metadata()?.is_file() {
		return Err(extension_error());
	}
	let mut bytes = Vec::new();
	file.take(65537).read_to_end(&mut bytes)?;
	if bytes.len() > 65536 {
		return Err(extension_error());
	}
	Ok(bytes)
}
pub(crate) fn digest(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
	let parent = path.parent().ok_or_else(extension_error)?;
	std::fs::create_dir_all(parent)?;
	let mut file = tempfile::NamedTempFile::new_in(parent)?;
	file.write_all(bytes)?;
	file.as_file().sync_all()?;
	file.persist(path).map_err(|_| extension_error())?;
	Ok(())
}
impl NativeConfig {
	pub(crate) fn value(&self, path: &Path) -> io::Result<Value> {
		self.parse(&read(path)?)
	}
	pub(crate) fn parse(&self, bytes: &[u8]) -> io::Result<Value> {
		if bytes.is_empty() {
			return Ok(serde_json::json!({}));
		}
		match self.format {
			NativeConfigFormat::Json => jet_protocol::decode_control(bytes)
				.map_err(|_| extension_error()),
			NativeConfigFormat::Toml => toml::from_str(
				std::str::from_utf8(bytes).map_err(|_| extension_error())?,
			)
			.map_err(|_| extension_error()),
		}
	}
	pub(crate) fn replace(
		&self,
		name: &str,
		entry: Option<&Value>,
	) -> io::Result<()> {
		let bytes = read(&self.path)?;
		let updated = match self.format {
			NativeConfigFormat::Json => {
				let mut value = self.value(&self.path)?;
				let root = value.as_object_mut().ok_or_else(extension_error)?;
				let collection = root
					.entry(self.key)
					.or_insert_with(|| serde_json::json!({}))
					.as_object_mut()
					.ok_or_else(extension_error)?;
				if let Some(entry) = entry {
					collection.insert(name.into(), entry.clone());
				} else {
					collection.remove(name);
				}
				serde_json::to_vec_pretty(&value)?
			}
			NativeConfigFormat::Toml => {
				let text = std::str::from_utf8(&bytes)
					.map_err(|_| extension_error())?;
				let mut doc: toml_edit::DocumentMut =
					text.parse().map_err(|_| extension_error())?;
				if !doc.contains_key(self.key) {
					doc[self.key] =
						toml_edit::Item::Table(toml_edit::Table::new());
				}
				let collection = doc[self.key]
					.as_table_like_mut()
					.ok_or_else(extension_error)?;
				if let Some(entry) = entry {
					let wrapper = serde_json::json!({"selected":entry});
					let mut native = toml_edit::ser::to_document(&wrapper)
						.map_err(|_| extension_error())?;
					collection.insert(
						name,
						native
							.remove("selected")
							.ok_or_else(extension_error)?,
					);
				} else {
					collection.remove(name);
				}
				doc.to_string().into_bytes()
			}
		};
		if updated.len() > 65536 {
			return Err(extension_error());
		}
		// Recheck the whole native document to avoid replacing unrelated concurrent edits.
		if read(&self.path)? != bytes {
			return Err(extension_error());
		}
		atomic_write(&self.path, &updated)
	}
}

pub(crate) fn file(path: &Path) -> io::Result<std::fs::File> {
	let file = std::fs::File::from(rustix::fs::open(
		path,
		rustix::fs::OFlags::RDONLY
			| rustix::fs::OFlags::NOFOLLOW
			| rustix::fs::OFlags::NONBLOCK
			| rustix::fs::OFlags::CLOEXEC,
		rustix::fs::Mode::empty(),
	)?);
	if !file.metadata()?.is_file() {
		return Err(extension_error());
	}
	Ok(file)
}
