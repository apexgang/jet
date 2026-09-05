//! A peer negotiated to a lower minor never sees what that minor does not
//! name (ADR-0019). The rule lives at this seam, so it is pinned here.

use std::time::{Duration, UNIX_EPOCH};

use jet_core::{
	AuditBreach, AuditEpoch, AuditHead, AuditSequence, EventSequence, PlaneId,
	PlaneStatus, ResolvedSetting, SecurityDegradation, SecurityState,
	SettingKey, SettingScope, SettingSnapshot, SettingSource, SettingValue,
};
use jet_protocol as wire;
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::{plane_status, setting};

fn degraded() -> PlaneStatus {
	PlaneStatus {
		cursor: EventSequence(4),
		plane_id: PlaneId(Uuid::nil()),
		daemon_starts: 2,
		started_at: UNIX_EPOCH + Duration::from_secs(1),
		core_version: "0.2.0",
		security: SecurityState::Degraded(SecurityDegradation {
			breach: AuditBreach::HeadNotInStore,
			epoch: AuditEpoch(1),
			head: Some(AuditHead {
				epoch: 1,
				sequence: 2,
				entry_hash: jet_core::AuditEntryHash([0; 32]),
			}),
			store_sequence: AuditSequence(1),
		}),
	}
}

fn resolved() -> SettingSnapshot {
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
		],
	}
}

#[test]
fn an_older_minor_is_neither_told_the_security_state_nor_its_setting() {
	let before = wire::SECURITY_AUDIT_MINOR - 1;

	assert_eq!(
		(
			plane_status(&degraded(), before).security,
			setting::snapshot(resolved(), before)
				.settings
				.into_iter()
				.map(|resolved| resolved.key)
				.collect::<Vec<_>>()
		),
		(None, vec![wire::SettingKey::UtilityAutomaticNaming])
	);
}

#[test]
fn the_minor_that_names_the_security_audit_is_told_both() {
	let minor = wire::SECURITY_AUDIT_MINOR;

	assert_eq!(
		(
			plane_status(&degraded(), minor).security.is_some(),
			setting::snapshot(resolved(), minor)
				.settings
				.into_iter()
				.map(|resolved| resolved.key)
				.collect::<Vec<_>>()
		),
		(
			true,
			vec![
				wire::SettingKey::UtilityAutomaticNaming,
				wire::SettingKey::SecurityAuditRetentionDays
			]
		)
	);
}
