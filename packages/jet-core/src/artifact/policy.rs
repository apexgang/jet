//! Resolve authenticated, durable ingestion settings over host defaults.
use crate::{
	ArtifactLimits, Core, CoreError, SettingKey, SettingScope, SettingSource,
	SettingValue,
};
impl Core {
	pub(crate) async fn artifact_policy(
		&self,
	) -> Result<ArtifactLimits, CoreError> {
		self.store
			.read(async |tx| self.artifact_policy_in(tx).await)
			.await
	}
	pub(crate) async fn artifact_policy_in(
		&self,
		tx: &mut jet_store::ReadTransaction,
	) -> Result<ArtifactLimits, CoreError> {
		let mut limits = self.artifact_limits;
		let settings =
			tx.settings_for_scope(SettingScope::Plane.record()).await?;
		for setting in crate::setting::resolve(
			&[SettingKey::ArtifactMaxMiB, SettingKey::ArtifactRunMiB],
			&settings,
		) {
			if setting.source == SettingSource::BuiltIn {
				continue;
			}
			let SettingValue::Count(mib) = setting.value else {
				return Err(crate::artifact::files::corrupt());
			};
			let bytes = u64::from(mib) * 1024 * 1024;
			if setting.key == SettingKey::ArtifactMaxMiB {
				limits.artifact_bytes = bytes;
			} else {
				limits.run_bytes = bytes;
			}
		}
		Ok(limits)
	}
}
