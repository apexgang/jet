//! Plane-local Account bindings and the opaque Credential references they
//! resolve through (ADR-0016, ADR-0076).
//!
//! Every Plane is authoritative for its own bindings: nothing here is
//! synchronized, and a Provider account exists only as the grouping a GUI
//! client makes from bindings that share a Provider-supplied identity.
//!
//! No column of this table can hold secret material. A binding records
//! which backend resolves its Credential, never the token, key, or password
//! that backend answers with.

use uuid::Uuid;

use crate::StoreError;
use crate::records::{column_error, parse_uuid};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// Which backend resolves one binding's Credential. The core owns what each
/// source means; the store keeps the durable spelling and the one piece of
/// non-secret text a source needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSourceRecord {
	/// The platform credential store, addressed by an item name the core
	/// derives from the binding identity.
	PlatformStore,
	/// An explicitly configured external authentication helper, named by
	/// the user and invoked at the moment of use.
	ExternalHelper {
		/// The helper's non-secret name.
		helper: String,
	},
	/// Native Harness authentication supplied by the environment the
	/// Harness is launched in. Jet holds no reference of its own.
	HarnessNative,
	/// Memory of one daemon process. A restart invalidates it.
	SessionOnly,
}

/// An Account binding to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccountBinding {
	/// Globally unique identity chosen by the caller.
	pub binding_id: Uuid,
	/// The Provider this binding authenticates to, such as `anthropic`.
	pub provider: String,
	/// The user-facing name of the binding.
	pub label: String,
	/// The Provider's own stable account identity, when it supplies one.
	pub provider_account: Option<String>,
	/// The backend that resolves the binding's Credential.
	pub credential: CredentialSourceRecord,
	/// The daemon start that established the binding, which decides whether
	/// a session-only Credential is still the one this process holds.
	pub established_at_daemon_start: u64,
	/// When the caller recorded the binding.
	pub created_at_unix_ms: i64,
}

/// One recorded Account binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBindingRecord {
	/// Globally unique identity.
	pub binding_id: Uuid,
	/// The Provider this binding authenticates to.
	pub provider: String,
	/// The user-facing name of the binding.
	pub label: String,
	/// The Provider's own stable account identity, when it supplies one.
	pub provider_account: Option<String>,
	/// The backend that resolves the binding's Credential.
	pub credential: CredentialSourceRecord,
	/// The daemon start that established the binding.
	pub established_at_daemon_start: u64,
	/// When the binding was recorded.
	pub created_at_unix_ms: i64,
}

/// One `account_bindings` row as SQLite stores it, before its text columns
/// are parsed back into domain types.
struct Row {
	binding_id: String,
	provider: String,
	label: String,
	provider_account: Option<String>,
	credential_source: String,
	credential_helper: Option<String>,
	established_at_daemon_start: i64,
	created_at_unix_ms: i64,
}

impl CredentialSourceRecord {
	/// The durable spelling of the source and the helper name it carries.
	fn columns(&self) -> (&'static str, Option<&str>) {
		match self {
			Self::PlatformStore => ("platform_store", None),
			Self::ExternalHelper { helper } => {
				("external_helper", Some(helper.as_str()))
			}
			Self::HarnessNative => ("harness_native", None),
			Self::SessionOnly => ("session_only", None),
		}
	}

	fn parse(source: &str, helper: Option<String>) -> Result<Self, StoreError> {
		match (source, helper) {
			("platform_store", None) => Ok(Self::PlatformStore),
			("external_helper", Some(helper)) => {
				Ok(Self::ExternalHelper { helper })
			}
			("harness_native", None) => Ok(Self::HarnessNative),
			("session_only", None) => Ok(Self::SessionOnly),
			(source, _) => Err(column_error(
				"credential_source",
				format!("unknown or incomplete Credential source {source:?}"),
			)),
		}
	}
}

impl ReadTransaction {
	/// Every Account binding on this Plane, in the order they were created.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn account_bindings(
		&mut self,
	) -> Result<Vec<AccountBindingRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query_as!(
			Row,
			r#"SELECT binding_id AS "binding_id!", provider, label,
				provider_account, credential_source, credential_helper,
				established_at_daemon_start, created_at_unix_ms
			 FROM account_bindings
			 ORDER BY rowid"#
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter().map(read_row).collect()
	}

	/// The Account binding identified by `binding_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn account_binding(
		&mut self,
		binding_id: Uuid,
	) -> Result<Option<AccountBindingRecord>, StoreError> {
		let binding_id = binding_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT binding_id AS "binding_id!", provider, label,
				provider_account, credential_source, credential_helper,
				established_at_daemon_start, created_at_unix_ms
			 FROM account_bindings
			 WHERE binding_id = ?1"#,
			binding_id
		)
		.fetch_optional(self.connection())
		.await?;
		row.map(read_row).transpose()
	}
}

impl WriteTransaction {
	/// Records a new Account binding and returns it as stored.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn insert_account_binding(
		&mut self,
		binding: NewAccountBinding,
	) -> Result<AccountBindingRecord, StoreError> {
		let NewAccountBinding {
			binding_id,
			provider,
			label,
			provider_account,
			credential,
			established_at_daemon_start,
			created_at_unix_ms,
		} = binding;
		let (source, helper) = credential.columns();
		let id = binding_id.to_string();
		let daemon_start =
			i64::try_from(established_at_daemon_start).map_err(|_| {
				column_error(
					"established_at_daemon_start",
					"the daemon start count does not fit the store".into(),
				)
			})?;
		sqlx::query!(
			"INSERT INTO account_bindings
				(binding_id, provider, label, provider_account,
				 credential_source, credential_helper,
				 established_at_daemon_start, created_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
			id,
			provider,
			label,
			provider_account,
			source,
			helper,
			daemon_start,
			created_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(AccountBindingRecord {
			binding_id,
			provider,
			label,
			provider_account,
			credential,
			established_at_daemon_start,
			created_at_unix_ms,
		})
	}

	/// Removes the Account binding `binding_id`. Removing one that is not
	/// recorded changes nothing.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be removed.
	pub async fn delete_account_binding(
		&mut self,
		binding_id: Uuid,
	) -> Result<(), StoreError> {
		let binding_id = binding_id.to_string();
		sqlx::query!(
			"DELETE FROM account_bindings WHERE binding_id = ?1",
			binding_id
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

fn read_row(row: Row) -> Result<AccountBindingRecord, StoreError> {
	Ok(AccountBindingRecord {
		binding_id: parse_uuid("binding_id", &row.binding_id)?,
		provider: row.provider,
		label: row.label,
		provider_account: row.provider_account,
		credential: CredentialSourceRecord::parse(
			&row.credential_source,
			row.credential_helper,
		)?,
		established_at_daemon_start: u64::try_from(
			row.established_at_daemon_start,
		)
		.map_err(|_| {
			column_error(
				"established_at_daemon_start",
				"the daemon start count is negative".into(),
			)
		})?,
		created_at_unix_ms: row.created_at_unix_ms,
	})
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
