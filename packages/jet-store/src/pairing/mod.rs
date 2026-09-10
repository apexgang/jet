//! The Plane's Pairing gate: whether a new GUI client may begin Pairing
//! with this Plane at all (ADR-0017).
//!
//! The gate is Plane-level and concerns new clients only. It is not an
//! answer about any client that is already Paired, and turning it off
//! leaves every existing pairing exactly as it was.

pub(crate) mod offer;
pub(crate) mod paired_client;

use crate::{
	StoreError,
	records::column_error,
	transaction::{ReadTransaction, WriteTransaction},
};
use serde::{Deserialize, Serialize};

/// Whether this Plane accepts new Pairings right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingGate {
	/// A new GUI client may begin Pairing.
	Open,
	/// It may not. A Plane starts here, so an owner opens the gate for as
	/// long as a pairing takes rather than leaving it open.
	Closed,
}

impl PairingGate {
	/// The durable spelling, also used in messages and JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Open => "open",
			Self::Closed => "closed",
		}
	}

	fn parse(state: &str) -> Result<Self, StoreError> {
		match state {
			"open" => Ok(Self::Open),
			"closed" => Ok(Self::Closed),
			state => Err(column_error(
				"state",
				format!("unknown Pairing gate state {state:?}"),
			)),
		}
	}
}

impl ReadTransaction {
	/// Whether the Plane accepts new Pairings, inside this transaction's
	/// consistent snapshot.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the gate cannot be read.
	pub async fn pairing_gate(&mut self) -> Result<PairingGate, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let state = sqlx::query_scalar!(
			"SELECT state FROM pairing_gate WHERE singleton = 1"
		)
		.fetch_one(self.connection())
		.await?;
		PairingGate::parse(&state)
	}
}

impl WriteTransaction {
	/// Records where the owner left the gate.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn set_pairing_gate(
		&mut self,
		gate: PairingGate,
		changed_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let state = gate.as_str();
		sqlx::query!(
			"UPDATE pairing_gate
			 SET state = ?1, changed_at_unix_ms = ?2
			 WHERE singleton = 1",
			state,
			changed_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;

	use super::PairingGate;
	use crate::Store;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	#[tokio::test]
	async fn the_gate_starts_closed_and_stays_where_the_owner_left_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");

		let store = Store::open(&path).await.unwrap();
		let new_plane = store
			.read(async |tx| tx.pairing_gate().await)
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.set_pairing_gate(PairingGate::Open, NOW_UNIX_MS).await
			})
			.await
			.unwrap();
		store.close().await;

		let reopened = Store::open(&path).await.unwrap();
		let after_restart = reopened
			.read(async |tx| tx.pairing_gate().await)
			.await
			.unwrap();

		assert_eq!(
			(new_plane, after_restart),
			(PairingGate::Closed, PairingGate::Open)
		);
	}
}
