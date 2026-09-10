//! Durable private plans for publishing accepted Craft installations.

use crate::{ReadTransaction, StoreError, WriteTransaction};
use uuid::Uuid;

impl ReadTransaction {
	/// Reads the last accepted publication and its settlement state.
	/// # Errors
	/// Returns a store error when SQLite cannot answer.
	pub async fn latest_craft_installation(
		&mut self,
		id: &str,
	) -> Result<Option<(String, String)>, StoreError> {
		Ok(sqlx::query!("SELECT p.plan, e.state FROM craft_installation_plans p JOIN effects e USING (effect_id) WHERE p.craft_id = ?1 AND e.state <> 'failed' ORDER BY p.rowid DESC LIMIT 1", id)
            .fetch_optional(self.connection()).await?.map(|row| (row.plan, row.state)))
	}

	/// Reads the bounded private plan owned by one Craft-install Effect.
	///
	/// # Errors
	/// Returns a [`StoreError`] when SQLite cannot answer.
	pub async fn craft_installation_plan(
		&mut self,
		effect_id: Uuid,
	) -> Result<Option<String>, StoreError> {
		let effect_id = effect_id.to_string();
		Ok(sqlx::query_scalar!(
			"SELECT plan FROM craft_installation_plans WHERE effect_id = ?1",
			effect_id
		)
		.fetch_optional(self.connection())
		.await?)
	}

	/// Reads all retained Craft-installation plans for Artifact marking.
	///
	/// # Errors
	/// Returns a [`StoreError`] when SQLite cannot answer.
	pub async fn craft_installation_plans(
		&mut self,
	) -> Result<Vec<String>, StoreError> {
		Ok(
			sqlx::query_scalar!("SELECT plan FROM craft_installation_plans")
				.fetch_all(self.connection())
				.await?,
		)
	}
}

impl WriteTransaction {
	/// Stores the immutable plan with the Effect that will publish it.
	///
	/// # Errors
	/// Returns a [`StoreError`] when the plan is oversized, its Effect is
	/// absent, or SQLite cannot write it.
	pub async fn insert_craft_installation_plan(
		&mut self,
		effect_id: Uuid,
		craft_id: &str,
		plan: &str,
	) -> Result<(), StoreError> {
		let effect_id = effect_id.to_string();
		sqlx::query!(
			"INSERT INTO craft_installation_plans (effect_id, craft_id, plan) \
			 VALUES (?1, ?2, ?3)",
			effect_id,
			craft_id,
			plan
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}
