//! Mutable Setting values and the scope rows one resolution reads
//! (ADR-0085).

use crate::StoreError;
use crate::records::{SettingRecord, SettingScopeRecord};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// One `settings` row as SQLite stores it, before its text columns are
/// parsed back into domain types.
struct Row {
	key: String,
	scope: String,
	scope_id: Option<String>,
	value: String,
	updated_at_unix_ms: i64,
}

impl ReadTransaction {
	/// Every stored Setting row that can apply to `scope`: the Plane's own
	/// values and the values recorded for the addressed scope itself. The
	/// core owns precedence between them and the built-in defaults beneath
	/// them.
	///
	/// A Conversation's Project values join this chain once Projects are
	/// registered; until then a Conversation resolves over the Plane alone.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn settings_for_scope(
		&mut self,
		scope: SettingScopeRecord,
	) -> Result<Vec<SettingRecord>, StoreError> {
		let (scope_kind, scope_id) = scope.columns();
		let scope_id = scope_id.map(|id| id.to_string());
		// A Plane scope binds no identity, and no stored Plane row has one,
		// so the second arm matches nothing and the chain is the Plane's.
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT key AS "key!", scope AS "scope!", scope_id, value,
				updated_at_unix_ms
			 FROM settings
			 WHERE scope = 'plane' OR (scope = ?1 AND scope_id = ?2)
			 ORDER BY key, scope"#,
			scope_kind,
			scope_id
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}
}

impl WriteTransaction {
	/// Records `setting` as the value its scope stores, replacing any value
	/// that scope stored before.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn upsert_setting(
		&mut self,
		setting: &SettingRecord,
	) -> Result<(), StoreError> {
		let (scope_kind, scope_id) = setting.scope.columns();
		let scope_id = scope_id.map(|id| id.to_string());
		// SQLite compares a NULL identity only through `IS`, which the
		// Plane row needs; `=` would never match it.
		let replaced = sqlx::query!(
			"UPDATE settings
			 SET value = ?4, updated_at_unix_ms = ?5
			 WHERE key = ?1 AND scope = ?2 AND scope_id IS ?3",
			setting.key,
			scope_kind,
			scope_id,
			setting.value,
			setting.updated_at_unix_ms
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		if replaced == 0 {
			sqlx::query!(
				"INSERT INTO settings
					(key, scope, scope_id, value, updated_at_unix_ms)
				 VALUES (?1, ?2, ?3, ?4, ?5)",
				setting.key,
				scope_kind,
				scope_id,
				setting.value,
				setting.updated_at_unix_ms
			)
			.execute(self.connection())
			.await?;
		}
		Ok(())
	}

	/// Removes the value `scope` stores for `key`, leaving the scopes above
	/// it untouched. Removing a value no scope stored changes nothing.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be removed.
	pub async fn delete_setting(
		&mut self,
		key: &str,
		scope: SettingScopeRecord,
	) -> Result<(), StoreError> {
		let (scope_kind, scope_id) = scope.columns();
		let scope_id = scope_id.map(|id| id.to_string());
		sqlx::query!(
			"DELETE FROM settings
			 WHERE key = ?1 AND scope = ?2 AND scope_id IS ?3",
			key,
			scope_kind,
			scope_id
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

fn read_row(row: Row) -> Result<SettingRecord, StoreError> {
	Ok(SettingRecord {
		key: row.key,
		scope: SettingScopeRecord::parse(&row.scope, row.scope_id.as_deref())?,
		value: row.value,
		updated_at_unix_ms: row.updated_at_unix_ms,
	})
}
