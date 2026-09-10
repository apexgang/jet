//! A peer negotiated to a lower minor never sees what that minor does not
//! name (ADR-0019). The rule lives at this seam, so it is pinned here.

use jet_core::{
	EventSequence, ResolvedSetting, SettingKey, SettingScope, SettingSnapshot,
	SettingSource, SettingValue,
};

pub(super) fn resolved() -> SettingSnapshot {
	SettingSnapshot {
		cursor: EventSequence(4),
		scope: SettingScope::Plane,
		settings: vec![
			ResolvedSetting {
				key: SettingKey::UtilityAutomaticNaming,
				value: SettingValue::Flag(true),
				source: SettingSource::BuiltIn,
			},
			ResolvedSetting {
				key: SettingKey::SecurityAuditRetentionDays,
				value: SettingValue::Count(365),
				source: SettingSource::BuiltIn,
			},
			ResolvedSetting {
				key: SettingKey::DeveloperMode,
				value: SettingValue::Flag(false),
				source: SettingSource::BuiltIn,
			},
		],
	}
}
