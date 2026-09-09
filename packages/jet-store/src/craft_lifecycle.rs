//! Durable admission barriers for disabled Crafts.
use crate::{ReadTransaction, StoreError, WriteTransaction};
/// Durable policy for an installed Craft identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CraftDisableMode {
	/// Refuse new Runs while allowing accepted Runs to finish.
	Wait,
	/// Also terminate resident Craft instances and prevent recovery.
	Force,
}
impl CraftDisableMode {
	fn from_force(force: bool) -> Self {
		if force { Self::Force } else { Self::Wait }
	}
}
impl ReadTransaction {
	/// Reads the permanent set of digests revoked by signed Jet metadata.
	/// # Errors
	/// Returns a store error when SQLite cannot answer.
	pub async fn craft_revocations(
		&mut self,
	) -> Result<Vec<String>, StoreError> {
		Ok(sqlx::query_scalar!("SELECT sha256 FROM craft_revocations")
			.fetch_all(self.connection())
			.await?)
	}
	/// Whether a digest is forbidden, independent of its Craft identity.
	/// # Errors
	/// Returns a store error when SQLite cannot answer.
	pub async fn craft_revoked(
		&mut self,
		digest: &str,
	) -> Result<bool, StoreError> {
		Ok(sqlx::query_scalar!(r#"SELECT EXISTS(SELECT 1 FROM craft_revocations WHERE sha256 = ?1) AS "revoked!: bool""#, digest).fetch_one(self.connection()).await?)
	}

	/// Reads disabled identities and whether their running Crafts must stop.
	/// # Errors
	/// Returns a store error if the snapshot cannot be read.
	pub async fn craft_disables(
		&mut self,
	) -> Result<Vec<(String, CraftDisableMode)>, StoreError> {
		Ok(sqlx::query!(
			r#"SELECT craft_id, force AS "force!: bool" FROM craft_disables"#
		)
		.fetch_all(self.connection())
		.await?
		.into_iter()
		.map(|r| (r.craft_id, CraftDisableMode::from_force(r.force)))
		.collect())
	}
	/// Reads the barrier for an identity; absence means enabled.
	/// # Errors
	/// Returns a store error when SQLite cannot answer.
	pub async fn craft_disabled(
		&mut self,
		id: &str,
	) -> Result<Option<CraftDisableMode>, StoreError> {
		Ok(sqlx::query_scalar!(
			r#"SELECT force AS "force!: bool" FROM craft_disables WHERE craft_id = ?1"#,
			id
		)
		.fetch_optional(self.connection())
		.await?
		.map(CraftDisableMode::from_force))
	}
}
impl WriteTransaction {
	/// Retains a verified digest revocation permanently.
	/// # Errors
	/// Returns a store error if the barrier cannot be committed.
	pub async fn revoke_craft_digest(
		&mut self,
		digest: &str,
	) -> Result<(), StoreError> {
		sqlx::query!(
			"INSERT INTO craft_revocations (sha256) VALUES (?1) ON CONFLICT DO NOTHING",
			digest
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Blocks future Runs. An existing Force disable cannot be downgraded.
	/// # Errors
	/// Returns a store error if the barrier cannot be committed.
	pub async fn disable_craft(
		&mut self,
		id: &str,
		mode: CraftDisableMode,
	) -> Result<(), StoreError> {
		let force = mode == CraftDisableMode::Force;
		sqlx::query!("INSERT INTO craft_disables (craft_id, force) VALUES (?1, ?2) ON CONFLICT(craft_id) DO UPDATE SET force = MAX(force, excluded.force)", id, force)
            .execute(self.connection()).await?;
		Ok(())
	}
}
