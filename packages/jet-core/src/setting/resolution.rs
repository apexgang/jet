//! Setting precedence and validation of mutation scopes.

use super::{
	ResolvedSetting, SettingKey, SettingScope, SettingSource, SettingValue,
};
use crate::{error::CoreError, event::EventSubject};
use jet_store::{ReadTransaction, SettingRecord};

/// Resolves `keys` for `scope` from the rows the scope chain stores,
/// narrowest scope winning and the built-in default beneath them all.
///
/// A row this core cannot read—an unknown key, or a value a later release
/// reshaped—resolves as if the scope stored nothing, so an older `jetd`
/// still answers from the scope above it (ADR-0073).
pub(crate) fn resolve(
	keys: &[SettingKey],
	stored: &[SettingRecord],
) -> Vec<ResolvedSetting> {
	keys.iter()
		.map(|&key| {
			let winner = stored
				.iter()
				.filter(|record| record.key == key.as_str())
				.filter_map(|record| {
					let scope = SettingScope::from_record(record.scope);
					let value = SettingValue::decode(&record.value)?;
					Some((scope, value))
				})
				.max_by_key(|(stored_scope, _)| stored_scope.kind());
			match winner {
				Some((stored_scope, value)) => ResolvedSetting {
					key,
					value,
					source: SettingSource::Scope(stored_scope),
				},
				None => ResolvedSetting {
					key,
					value: key.built_in(),
					source: SettingSource::BuiltIn,
				},
			}
		})
		.collect()
}

/// Validates a Setting write and encodes its value for the store.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the scope may not store
/// the Setting or the value does not fit it.
pub(crate) fn prepare_write(
	key: SettingKey,
	scope: SettingScope,
	value: &SettingValue,
) -> Result<String, CoreError> {
	key.require_scope(scope)?;
	key.require_value(value)?;
	value.encode()
}

/// Validates removing whatever a scope stores for a Setting.
///
/// # Errors
///
/// Returns an `invalid_input` [`CoreError`] when the scope may not store
/// the Setting.
pub(crate) fn prepare_clear(
	key: SettingKey,
	scope: SettingScope,
) -> Result<(), CoreError> {
	key.require_scope(scope)
}

/// What the journal files a change to `scope` under. A Conversation's
/// Settings belong to its history; the Plane's and a Project's belong to
/// the Plane.
pub(crate) fn event_subject(scope: SettingScope) -> EventSubject {
	match scope {
		SettingScope::Plane | SettingScope::Project { .. } => {
			EventSubject::Plane
		}
		SettingScope::Conversation { conversation_id } => {
			EventSubject::Conversation(conversation_id)
		}
	}
}

/// The value the Plane itself resolves for `key`: whatever the Plane scope
/// stores, and the built-in default until something does.
///
/// # Errors
///
/// Returns a store category [`CoreError`] when the values cannot be read.
pub(crate) async fn resolve_plane(
	tx: &mut ReadTransaction,
	key: SettingKey,
) -> Result<SettingValue, CoreError> {
	let stored = tx.settings_for_scope(SettingScope::Plane.record()).await?;
	Ok(resolve(&[key], &stored)
		.into_iter()
		.next()
		.map_or_else(|| key.built_in(), |resolved| resolved.value))
}

/// Refuses a scope whose subject this Plane does not have.
///
/// # Errors
///
/// Returns a `not_found` [`CoreError`] when the named Project or
/// Conversation does not exist, or a store category when the check cannot
/// be answered.
pub(crate) async fn require_subject(
	tx: &mut ReadTransaction,
	scope: SettingScope,
) -> Result<(), CoreError> {
	match scope {
		SettingScope::Plane => Ok(()),
		SettingScope::Project { project_id } => {
			if tx.project(project_id.0).await?.is_some() {
				Ok(())
			} else {
				Err(CoreError::not_found(
					"project.not_found",
					"the Project does not exist",
				))
			}
		}
		SettingScope::Conversation { conversation_id } => {
			if tx.conversation(conversation_id.0).await?.is_some() {
				Ok(())
			} else {
				Err(CoreError::not_found(
					"conversation.not_found",
					"the Conversation does not exist",
				))
			}
		}
	}
}
