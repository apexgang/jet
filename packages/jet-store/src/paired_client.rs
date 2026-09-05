//! The GUI clients this Plane has Paired with (ADR-0017).
//!
//! A Paired client is a Client identity the Plane accepts, its durable
//! public key, and whether it may control the Plane right now. The key is
//! the public half of an identity whose private half never leaves the
//! installation that generated it, so nothing here is secret.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StoreError;
use crate::pairing_offer::PairingKeyAlgorithm;
use crate::records::{column_error, parse_uuid};
use crate::transaction::{ReadTransaction, WriteTransaction};

/// Whether a Paired client may control the Plane right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedClientAccess {
	/// It may.
	Enabled,
	/// It may not. The Plane keeps its key, so enabling it again needs no
	/// new pairing.
	Disabled,
}

/// One Paired client to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPairedClient {
	/// The Client identity that was Paired.
	pub client_id: Uuid,
	/// The algorithm its durable key signs with.
	pub key_algorithm: PairingKeyAlgorithm,
	/// The durable public key that is now the credential.
	pub public_key: [u8; 32],
	/// The Pairing protocol the key was established under.
	pub pairing_protocol: String,
	/// When the Pairing completed.
	pub paired_at_unix_ms: i64,
}

/// One recorded Paired client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedClientRecord {
	/// The Client identity.
	pub client_id: Uuid,
	/// The algorithm its durable key signs with.
	pub key_algorithm: PairingKeyAlgorithm,
	/// The durable public key.
	pub public_key: [u8; 32],
	/// The Pairing protocol the key was established under.
	pub pairing_protocol: String,
	/// Whether it may control the Plane right now.
	pub access: PairedClientAccess,
	/// When the Pairing completed.
	pub paired_at_unix_ms: i64,
}

impl PairedClientAccess {
	/// The durable spelling, also used in messages and JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Enabled => "enabled",
			Self::Disabled => "disabled",
		}
	}

	fn parse(access: &str) -> Result<Self, StoreError> {
		match access {
			"enabled" => Ok(Self::Enabled),
			"disabled" => Ok(Self::Disabled),
			access => Err(column_error(
				"access",
				format!("unknown Paired client access {access:?}"),
			)),
		}
	}
}

impl ReadTransaction {
	/// Every Paired client on this Plane, in the order they were Paired.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn paired_clients(
		&mut self,
	) -> Result<Vec<PairedClientRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let rows = sqlx::query!(
			r#"SELECT client_id AS "client_id!", key_algorithm, public_key,
				pairing_protocol, access, paired_at_unix_ms
			 FROM paired_clients
			 ORDER BY paired_at_unix_ms, rowid"#
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter()
			.map(|row| {
				Ok(PairedClientRecord {
					client_id: parse_uuid("client_id", &row.client_id)?,
					key_algorithm: PairingKeyAlgorithm::parse(
						&row.key_algorithm,
					)?,
					public_key: parse_public_key(row.public_key)?,
					pairing_protocol: row.pairing_protocol,
					access: PairedClientAccess::parse(&row.access)?,
					paired_at_unix_ms: row.paired_at_unix_ms,
				})
			})
			.collect()
	}
}

impl WriteTransaction {
	/// Records a completed Pairing, replacing whatever this Plane held for
	/// that Client identity. Pairing again is how a client that lost its
	/// key comes back, so it is one row per client rather than a second
	/// pairing beside the first.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn upsert_paired_client(
		&mut self,
		client: NewPairedClient,
	) -> Result<PairedClientRecord, StoreError> {
		let id = client.client_id.to_string();
		let algorithm = client.key_algorithm.as_str();
		let public_key = client.public_key.as_slice();
		let access = PairedClientAccess::Enabled.as_str();
		sqlx::query!(
			"INSERT INTO paired_clients
				(client_id, key_algorithm, public_key, pairing_protocol,
				 access, paired_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
			 ON CONFLICT (client_id) DO UPDATE SET
				key_algorithm = excluded.key_algorithm,
				public_key = excluded.public_key,
				pairing_protocol = excluded.pairing_protocol,
				access = excluded.access,
				paired_at_unix_ms = excluded.paired_at_unix_ms",
			id,
			algorithm,
			public_key,
			client.pairing_protocol,
			access,
			client.paired_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		let NewPairedClient {
			client_id,
			key_algorithm,
			public_key,
			pairing_protocol,
			paired_at_unix_ms,
		} = client;
		Ok(PairedClientRecord {
			client_id,
			key_algorithm,
			public_key,
			pairing_protocol,
			access: PairedClientAccess::Enabled,
			paired_at_unix_ms,
		})
	}
}

fn parse_public_key(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
	let length = bytes.len();
	bytes.try_into().map_err(|_| {
		column_error("public_key", format!("the key has {length} bytes"))
	})
}

#[cfg(test)]
#[path = "paired_client_tests.rs"]
mod tests;
