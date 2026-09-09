//! Publication and removal of the already inspected native contents.
use super::*;
use std::{io::Read, os::unix::fs::PermissionsExt};
impl StandaloneExtensions {
	pub(super) fn change_skill(
		&self,
		selected: &Target<'_>,
		action: ExtensionAction,
		confirmed: &Value,
	) -> io::Result<()> {
		let destination = self.skills.join(selected.name);
		let disabled = self.disabled_skills().join(selected.name);
		match action {
			ExtensionAction::Install | ExtensionAction::Update => {
				let source = selected.source.as_deref().unwrap_or(&disabled);
				let files = extension_files(source)?;
				std::fs::create_dir_all(&self.skills)?;
				let staged = tempfile::tempdir_in(
					self.skills.parent().ok_or_else(extension_error)?,
				)?;
				let replacement = staged.path().join("skill");
				std::fs::create_dir(&replacement)?;
				let mut remaining = 16 * 1024 * 1024;
				for file in files {
					let from = source.join(&file.path);
					let to = replacement.join(&file.path);
					std::fs::create_dir_all(
						to.parent().ok_or_else(extension_error)?,
					)?;
					let input = native_config::file(&from)?;
					let mode = input.metadata()?.permissions().mode() & 0o777;
					let mut output = std::fs::File::create(&to)?;
					let copied =
						io::copy(&mut input.take(remaining + 1), &mut output)?;
					remaining = remaining
						.checked_sub(copied)
						.ok_or_else(extension_error)?;
					output.set_permissions(std::fs::Permissions::from_mode(
						mode,
					))?;
					output.sync_all()?;
				}
				if serde_json::to_value(extension_files(&replacement)?)?
					!= confirmed["files"]
				{
					return Err(extension_error());
				}
				let current: Value = serde_json::from_str(
					&self
						.inspect(
							confirmed["extension_id"]
								.as_str()
								.ok_or_else(extension_error)?,
						)?
						.native_metadata,
				)?;
				if &current != confirmed {
					return Err(extension_error());
				}
				let previous = staged.path().join("previous");
				if destination.exists() {
					std::fs::rename(&destination, &previous)?;
				}
				if let Err(error) = std::fs::rename(replacement, &destination) {
					if previous.exists()
						&& std::fs::rename(&previous, &destination).is_err()
					{
						let _preserved = staged.keep();
					}
					return Err(error);
				}
				if disabled.exists() {
					std::fs::remove_dir_all(disabled)?;
				}
			}
			ExtensionAction::Disable => {
				if disabled.exists() {
					return Err(extension_error());
				}
				std::fs::create_dir_all(self.disabled_skills())?;
				std::fs::rename(destination, disabled)?;
			}
			ExtensionAction::Remove => {
				if destination.exists() {
					std::fs::remove_dir_all(destination)?;
				}
				if disabled.exists() {
					std::fs::remove_dir_all(disabled)?;
				}
			}
		}
		Ok(())
	}
	pub(super) fn change_config(
		&self,
		selected: &Target<'_>,
		action: ExtensionAction,
		confirmed: &Value,
	) -> io::Result<()> {
		let config = self.config(selected.kind)?;
		let backup = NativeConfig {
			path: self.disabled_config(config),
			..config.clone()
		};
		if native_config::digest(&native_config::read(&config.path)?)
			!= confirmed["current"]["sha256"].as_str().unwrap_or_default()
			|| native_config::digest(&native_config::read(&backup.path)?)
				!= confirmed["current"]["disabled_sha256"]
					.as_str()
					.unwrap_or_default()
		{
			return Err(extension_error());
		}
		match action {
			ExtensionAction::Install | ExtensionAction::Update => {
				let source = selected.source.as_deref().unwrap_or(&backup.path);
				let bytes = native_config::read(source)?;
				if native_config::digest(&bytes)
					!= confirmed["files"][0]["sha256"]
						.as_str()
						.unwrap_or_default()
				{
					return Err(extension_error());
				}
				let entry =
					config.parse(&bytes)?[config.key][selected.name].clone();
				config.replace(selected.name, Some(&entry))?;
				backup.replace(selected.name, None)?;
			}
			ExtensionAction::Disable => {
				let entry = config.value(&config.path)?[config.key]
					[selected.name]
					.clone();
				if entry.is_null() {
					return Err(extension_error());
				}
				backup.replace(selected.name, Some(&entry))?;
				config.replace(selected.name, None)?;
			}
			ExtensionAction::Remove => {
				config.replace(selected.name, None)?;
				backup.replace(selected.name, None)?;
			}
		}
		Ok(())
	}
}
