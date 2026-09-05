//! The Setting half of the translation seam (ADR-0049, ADR-0085).

use jet_core::{
	ConversationId, ProjectId, ResolvedSetting, SettingKey, SettingScope,
	SettingSelection, SettingSnapshot, SettingSource, SettingValue,
};
use jet_protocol as wire;

pub(super) fn snapshot(snapshot: SettingSnapshot) -> wire::SettingSnapshot {
	wire::SettingSnapshot {
		cursor: snapshot.cursor.0,
		scope: scope(snapshot.scope),
		settings: snapshot
			.settings
			.into_iter()
			.map(resolved_setting)
			.collect(),
	}
}

fn resolved_setting(resolved: ResolvedSetting) -> wire::ResolvedSetting {
	wire::ResolvedSetting {
		key: key(resolved.key),
		value: value(resolved.value),
		source: match resolved.source {
			SettingSource::BuiltIn => wire::SettingSource::BuiltIn,
			SettingSource::Scope(stored) => wire::SettingSource::Scope {
				scope: scope(stored),
			},
		},
	}
}

pub(super) fn key(key: SettingKey) -> wire::SettingKey {
	match key {
		SettingKey::UtilityAutomaticNaming => {
			wire::SettingKey::UtilityAutomaticNaming
		}
		SettingKey::GitAutoCommit => wire::SettingKey::GitAutoCommit,
		SettingKey::GitMessageInstructions => {
			wire::SettingKey::GitMessageInstructions
		}
	}
}

pub(super) fn key_from_wire(key: wire::SettingKey) -> SettingKey {
	match key {
		wire::SettingKey::UtilityAutomaticNaming => {
			SettingKey::UtilityAutomaticNaming
		}
		wire::SettingKey::GitAutoCommit => SettingKey::GitAutoCommit,
		wire::SettingKey::GitMessageInstructions => {
			SettingKey::GitMessageInstructions
		}
	}
}

pub(super) fn value(value: SettingValue) -> wire::SettingValue {
	match value {
		SettingValue::Flag(flag) => wire::SettingValue::Flag(flag),
		SettingValue::Text(text) => wire::SettingValue::Text(text),
	}
}

pub(super) fn value_from_wire(value: wire::SettingValue) -> SettingValue {
	match value {
		wire::SettingValue::Flag(flag) => SettingValue::Flag(flag),
		wire::SettingValue::Text(text) => SettingValue::Text(text),
	}
}

pub(super) fn scope(scope: SettingScope) -> wire::SettingScope {
	match scope {
		SettingScope::Plane => wire::SettingScope::Plane,
		SettingScope::Project { project_id } => wire::SettingScope::Project {
			project_id: project_id.0,
		},
		SettingScope::Conversation { conversation_id } => {
			wire::SettingScope::Conversation {
				conversation_id: conversation_id.0,
			}
		}
	}
}

pub(super) fn scope_from_wire(scope: wire::SettingScope) -> SettingScope {
	match scope {
		wire::SettingScope::Plane => SettingScope::Plane,
		wire::SettingScope::Project { project_id } => SettingScope::Project {
			project_id: ProjectId(project_id),
		},
		wire::SettingScope::Conversation { conversation_id } => {
			SettingScope::Conversation {
				conversation_id: ConversationId(conversation_id),
			}
		}
	}
}

pub(super) fn selection_from_wire(
	selection: wire::SettingSelection,
) -> SettingSelection {
	match selection {
		wire::SettingSelection::All => SettingSelection::All,
		wire::SettingSelection::Key { key } => {
			SettingSelection::Key(key_from_wire(key))
		}
	}
}
