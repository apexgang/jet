//! The one Pairing offer a Plane may have open, and the claim a GUI client
//! makes against it (ADR-0017).
//!
//! A Plane pairs with one client at a time, so this is a single row:
//! opening an offer replaces whatever was open. No column can hold the
//! offer's one-time secret. What is stored is a salted digest of it, so a
//! store that is read cannot be used to claim the offer it describes.

use crate::{
	StoreError,
	records::{column_error, parse_bytes, parse_optional_uuid, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How the Plane hands one offer's one-time secret to the person pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum PairingMethod {
	/// An eight-digit numeric code the owner reads off the target and the
	/// person pairing types into the GUI client.
	ManualCode,
	/// A versioned payload a GUI client scans, carrying the endpoint it
	/// should reach the Plane at beside the offer's one-time token.
	QrPayload {
		/// The reachable endpoint the payload advertises.
		endpoint: String,
	},
}

/// How far one offer has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingOfferState {
	/// Issued, and waiting for a client to present its secret.
	Offered,
	/// Claimed by a client. Both sides now show the authentication string,
	/// and the pairing waits for the people at each end.
	AwaitingConfirmation,
	/// Dead. It cannot be claimed or confirmed, only replaced.
	Invalidated {
		/// Why it stopped being usable. An invalidated offer always has
		/// one, which is why it is held here rather than beside the state.
		reason: PairingInvalidation,
	},
}

/// Why an offer stopped being usable before it was completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingInvalidation {
	/// Too many wrong secrets were presented against it.
	TooManyAttempts,
	/// The owner closed the Plane's Pairing gate while it was open.
	GateClosed,
}

/// The signature algorithm one Client identity signs with. Stored beside
/// the key so a Plane that later speaks a second algorithm can tell which
/// key is which (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingKeyAlgorithm {
	/// Ed25519, the only algorithm a v1 Client identity uses.
	Ed25519,
}

/// One Pairing offer to record, replacing whatever the Plane had open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPairingOffer {
	/// Globally unique identity chosen by the caller.
	pub offer_id: Uuid,
	/// How its secret reaches the person pairing.
	pub method: PairingMethod,
	/// The salt its digest was taken over.
	pub secret_salt: [u8; 16],
	/// The digest of its one-time secret. The secret itself is disclosed
	/// once, to the owner who opened the offer, and never stored.
	pub secret_digest: [u8; 32],
	/// The Client identity of the owner that opened it.
	pub opened_by: Uuid,
	/// When it was opened.
	pub opened_at_unix_ms: i64,
	/// When the secret stops being accepted.
	pub expires_at_unix_ms: i64,
}

/// What a claiming client presented, and what the Plane answered it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPairingClaim {
	/// The Client identity that claimed the offer.
	pub client_id: Uuid,
	/// The algorithm its durable key signs with.
	pub key_algorithm: PairingKeyAlgorithm,
	/// The durable public key that becomes the credential once Pairing
	/// completes.
	pub public_key: [u8; 32],
	/// The fresh challenge the Plane issued for that key to sign.
	pub challenge: [u8; 32],
	/// The string both sides display for the people to compare.
	pub authentication_string: String,
}

/// One recorded Pairing offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingOfferRecord {
	/// Globally unique identity.
	pub offer_id: Uuid,
	/// How its secret reached the person pairing.
	pub method: PairingMethod,
	/// The salt its digest was taken over.
	pub secret_salt: [u8; 16],
	/// The digest of its one-time secret.
	pub secret_digest: [u8; 32],
	/// How far it has got, and why it stopped if it has.
	pub state: PairingOfferState,
	/// How many wrong secrets have been presented against it.
	pub failed_attempts: u32,
	/// The Client identity of the owner that opened it.
	pub opened_by: Uuid,
	/// When it was opened.
	pub opened_at_unix_ms: i64,
	/// When its current step stops being accepted.
	pub expires_at_unix_ms: i64,
	/// What the claiming client presented, once one has.
	pub claim: Option<NewPairingClaim>,
	/// Who confirmed that both screens show the same authentication string,
	/// once somebody has.
	pub confirmed_by: Option<Uuid>,
}

impl PairingMethod {
	/// The durable spelling of the method and the endpoint it carries.
	fn columns(&self) -> (&'static str, Option<&str>) {
		match self {
			Self::ManualCode => ("manual_code", None),
			Self::QrPayload { endpoint } => {
				("qr_payload", Some(endpoint.as_str()))
			}
		}
	}

	fn parse(
		method: &str,
		endpoint: Option<String>,
	) -> Result<Self, StoreError> {
		match (method, endpoint) {
			("manual_code", None) => Ok(Self::ManualCode),
			("qr_payload", Some(endpoint)) => Ok(Self::QrPayload { endpoint }),
			(method, _) => Err(column_error(
				"method",
				format!("unknown or incomplete Pairing method {method:?}"),
			)),
		}
	}
}

impl PairingOfferState {
	/// The durable spelling of the state and the reason it carries.
	fn columns(self) -> (&'static str, Option<&'static str>) {
		match self {
			Self::Offered => ("offered", None),
			Self::AwaitingConfirmation => ("awaiting_confirmation", None),
			Self::Invalidated { reason } => {
				("invalidated", Some(reason.as_str()))
			}
		}
	}

	fn parse(
		state: &str,
		invalidation: Option<&str>,
	) -> Result<Self, StoreError> {
		match (state, invalidation) {
			("offered", None) => Ok(Self::Offered),
			("awaiting_confirmation", None) => Ok(Self::AwaitingConfirmation),
			("invalidated", Some(reason)) => Ok(Self::Invalidated {
				reason: PairingInvalidation::parse(reason)?,
			}),
			(state, _) => Err(column_error(
				"state",
				format!("unknown or incomplete Pairing offer state {state:?}"),
			)),
		}
	}
}

impl PairingInvalidation {
	/// The durable spelling, also used in messages and JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::TooManyAttempts => "too_many_attempts",
			Self::GateClosed => "gate_closed",
		}
	}

	fn parse(invalidation: &str) -> Result<Self, StoreError> {
		match invalidation {
			"too_many_attempts" => Ok(Self::TooManyAttempts),
			"gate_closed" => Ok(Self::GateClosed),
			invalidation => Err(column_error(
				"invalidation",
				format!("unknown Pairing invalidation {invalidation:?}"),
			)),
		}
	}
}

impl PairingKeyAlgorithm {
	/// The durable spelling, also used in messages and JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Ed25519 => "ed25519",
		}
	}

	pub(crate) fn parse(algorithm: &str) -> Result<Self, StoreError> {
		match algorithm {
			"ed25519" => Ok(Self::Ed25519),
			algorithm => Err(column_error(
				"key_algorithm",
				format!("unknown Pairing key algorithm {algorithm:?}"),
			)),
		}
	}
}

impl ReadTransaction {
	/// The Pairing offer this Plane has open, if any, inside this
	/// transaction's consistent snapshot.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn pairing_offer(
		&mut self,
	) -> Result<Option<PairingOfferRecord>, StoreError> {
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let row = sqlx::query!(
			r#"SELECT offer_id, method, endpoint, secret_salt, secret_digest,
				state, invalidation, failed_attempts, opened_by,
				opened_at_unix_ms, expires_at_unix_ms, claimed_by,
				key_algorithm, public_key, challenge, authentication_string,
				confirmed_by
			 FROM pairing_offers
			 WHERE singleton = 1"#
		)
		.fetch_optional(self.connection())
		.await?;
		let Some(row) = row else {
			return Ok(None);
		};
		let claimed_by =
			parse_optional_uuid("claimed_by", row.claimed_by.as_deref())?;
		let claim = match (
			claimed_by,
			row.key_algorithm,
			row.public_key,
			row.challenge,
			row.authentication_string,
		) {
			(
				Some(client_id),
				Some(algorithm),
				Some(public_key),
				Some(challenge),
				Some(authentication_string),
			) => Some(NewPairingClaim {
				client_id,
				key_algorithm: PairingKeyAlgorithm::parse(&algorithm)?,
				public_key: parse_bytes("public_key", public_key)?,
				challenge: parse_bytes("challenge", challenge)?,
				authentication_string,
			}),
			_ => None,
		};
		Ok(Some(PairingOfferRecord {
			offer_id: parse_uuid("offer_id", &row.offer_id)?,
			method: PairingMethod::parse(&row.method, row.endpoint)?,
			secret_salt: parse_bytes("secret_salt", row.secret_salt)?,
			secret_digest: parse_bytes("secret_digest", row.secret_digest)?,
			state: PairingOfferState::parse(
				&row.state,
				row.invalidation.as_deref(),
			)?,
			failed_attempts: u32::try_from(row.failed_attempts).map_err(
				|_| {
					column_error(
						"failed_attempts",
						"the attempt count is out of range".into(),
					)
				},
			)?,
			opened_by: parse_uuid("opened_by", &row.opened_by)?,
			opened_at_unix_ms: row.opened_at_unix_ms,
			expires_at_unix_ms: row.expires_at_unix_ms,
			claim,
			confirmed_by: parse_optional_uuid(
				"confirmed_by",
				row.confirmed_by.as_deref(),
			)?,
		}))
	}
}

impl WriteTransaction {
	/// Records `offer` as the one offer this Plane has open, replacing
	/// whatever was open before it, and returns it as stored.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn replace_pairing_offer(
		&mut self,
		offer: NewPairingOffer,
	) -> Result<PairingOfferRecord, StoreError> {
		sqlx::query!("DELETE FROM pairing_offers")
			.execute(self.connection())
			.await?;
		let (method, endpoint) = offer.method.columns();
		let offer_id = offer.offer_id.to_string();
		let salt = offer.secret_salt.as_slice();
		let digest = offer.secret_digest.as_slice();
		let opened_by = offer.opened_by.to_string();
		let (state, _) = PairingOfferState::Offered.columns();
		sqlx::query!(
			"INSERT INTO pairing_offers
				(singleton, offer_id, method, endpoint, secret_salt,
				 secret_digest, state, failed_attempts, opened_by,
				 opened_at_unix_ms, expires_at_unix_ms)
			 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?9)",
			offer_id,
			method,
			endpoint,
			salt,
			digest,
			state,
			opened_by,
			offer.opened_at_unix_ms,
			offer.expires_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		let NewPairingOffer {
			offer_id,
			method,
			secret_salt,
			secret_digest,
			opened_by,
			opened_at_unix_ms,
			expires_at_unix_ms,
		} = offer;
		Ok(PairingOfferRecord {
			offer_id,
			method,
			secret_salt,
			secret_digest,
			state: PairingOfferState::Offered,
			failed_attempts: 0,
			opened_by,
			opened_at_unix_ms,
			expires_at_unix_ms,
			claim: None,
			confirmed_by: None,
		})
	}

	/// Records that `confirmed_by` agreed the authentication string on both
	/// screens is the same one, and gives the client its own window to
	/// prove its key in.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn record_pairing_confirmation(
		&mut self,
		confirmed_by: Uuid,
		expires_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let confirmed_by = confirmed_by.to_string();
		sqlx::query!(
			"UPDATE pairing_offers
			 SET confirmed_by = ?1, expires_at_unix_ms = ?2
			 WHERE singleton = 1",
			confirmed_by,
			expires_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Forgets the open offer, which a completed Pairing has no further use
	/// for: what it established is the Paired client it left behind.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be removed.
	pub async fn delete_pairing_offer(&mut self) -> Result<(), StoreError> {
		sqlx::query!("DELETE FROM pairing_offers")
			.execute(self.connection())
			.await?;
		Ok(())
	}

	/// Records what a claiming client presented and gives the offer its own
	/// window to be confirmed in.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn record_pairing_claim(
		&mut self,
		claim: &NewPairingClaim,
		expires_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let (state, _) = PairingOfferState::AwaitingConfirmation.columns();
		let client_id = claim.client_id.to_string();
		let algorithm = claim.key_algorithm.as_str();
		let public_key = claim.public_key.as_slice();
		let challenge = claim.challenge.as_slice();
		sqlx::query!(
			"UPDATE pairing_offers
			 SET state = ?1, claimed_by = ?2, key_algorithm = ?3,
				 public_key = ?4, challenge = ?5, authentication_string = ?6,
				 expires_at_unix_ms = ?7
			 WHERE singleton = 1",
			state,
			client_id,
			algorithm,
			public_key,
			challenge,
			claim.authentication_string,
			expires_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Counts one wrong secret against the open offer and returns how many
	/// have now been presented against it.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the count cannot be written.
	pub async fn record_failed_pairing_attempt(
		&mut self,
	) -> Result<u32, StoreError> {
		let attempts = sqlx::query_scalar!(
			"UPDATE pairing_offers
			 SET failed_attempts = failed_attempts + 1
			 WHERE singleton = 1
			 RETURNING failed_attempts"
		)
		.fetch_one(self.connection())
		.await?;
		u32::try_from(attempts).map_err(|_| {
			column_error(
				"failed_attempts",
				"the attempt count is out of range".into(),
			)
		})
	}

	/// Kills the open offer, recording why. An offer that is already dead
	/// keeps the reason it died of.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written.
	pub async fn invalidate_pairing_offer(
		&mut self,
		reason: PairingInvalidation,
	) -> Result<(), StoreError> {
		let (state, invalidation) =
			PairingOfferState::Invalidated { reason }.columns();
		sqlx::query!(
			"UPDATE pairing_offers
			 SET state = ?1, invalidation = ?2
			 WHERE singleton = 1 AND state <> ?1",
			state,
			invalidation
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		NewPairingClaim, NewPairingOffer, PairingInvalidation,
		PairingKeyAlgorithm, PairingMethod, PairingOfferRecord,
		PairingOfferState,
	};
	use crate::Store;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;
	const EXPIRES_UNIX_MS: i64 = NOW_UNIX_MS + 120_000;

	fn offer(method: PairingMethod) -> NewPairingOffer {
		NewPairingOffer {
			offer_id: Uuid::now_v7(),
			method,
			secret_salt: [7; 16],
			secret_digest: [9; 32],
			opened_by: Uuid::now_v7(),
			opened_at_unix_ms: NOW_UNIX_MS,
			expires_at_unix_ms: EXPIRES_UNIX_MS,
		}
	}

	fn claim() -> NewPairingClaim {
		NewPairingClaim {
			client_id: Uuid::now_v7(),
			key_algorithm: PairingKeyAlgorithm::Ed25519,
			public_key: [3; 32],
			challenge: [4; 32],
			authentication_string: "418-273".into(),
		}
	}

	fn recorded(
		offer: &NewPairingOffer,
		state: PairingOfferState,
		claim: Option<NewPairingClaim>,
	) -> PairingOfferRecord {
		PairingOfferRecord {
			offer_id: offer.offer_id,
			method: offer.method.clone(),
			secret_salt: offer.secret_salt,
			secret_digest: offer.secret_digest,
			state,
			failed_attempts: 0,
			opened_by: offer.opened_by,
			opened_at_unix_ms: offer.opened_at_unix_ms,
			expires_at_unix_ms: offer.expires_at_unix_ms,
			claim,
			confirmed_by: None,
		}
	}

	#[tokio::test]
	async fn opening_an_offer_replaces_the_one_the_plane_had_open() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let first = offer(PairingMethod::ManualCode);
		let second = offer(PairingMethod::QrPayload {
			endpoint: "alex@studio.example".into(),
		});

		let none_yet = store
			.read(async |tx| tx.pairing_offer().await)
			.await
			.unwrap();
		store
			.write(async |tx| tx.replace_pairing_offer(first.clone()).await)
			.await
			.unwrap();
		store
			.write(async |tx| tx.replace_pairing_offer(second.clone()).await)
			.await
			.unwrap();
		let open = store
			.read(async |tx| tx.pairing_offer().await)
			.await
			.unwrap();

		assert_eq!(
			(none_yet, open),
			(
				None,
				Some(recorded(&second, PairingOfferState::Offered, None))
			)
		);
	}

	#[tokio::test]
	async fn a_claim_and_its_failed_attempts_are_recorded_against_the_offer() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let offered = offer(PairingMethod::ManualCode);
		let claim = claim();

		store
			.write(async |tx| tx.replace_pairing_offer(offered.clone()).await)
			.await
			.unwrap();
		let attempts = store
			.write(async |tx| {
				tx.record_failed_pairing_attempt().await?;
				tx.record_failed_pairing_attempt().await
			})
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.record_pairing_claim(&claim, EXPIRES_UNIX_MS + 120_000)
					.await
			})
			.await
			.unwrap();
		let claimed = store
			.read(async |tx| tx.pairing_offer().await)
			.await
			.unwrap();

		assert_eq!(
			(attempts, claimed),
			(
				2,
				Some(PairingOfferRecord {
					failed_attempts: 2,
					expires_at_unix_ms: EXPIRES_UNIX_MS + 120_000,
					..recorded(
						&offered,
						PairingOfferState::AwaitingConfirmation,
						Some(claim)
					)
				})
			)
		);
	}

	#[tokio::test]
	async fn an_invalidated_offer_keeps_the_reason_it_died_of() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let offered = offer(PairingMethod::ManualCode);

		store
			.write(async |tx| tx.replace_pairing_offer(offered.clone()).await)
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.invalidate_pairing_offer(
					PairingInvalidation::TooManyAttempts,
				)
				.await
			})
			.await
			.unwrap();
		store
			.write(async |tx| {
				tx.invalidate_pairing_offer(PairingInvalidation::GateClosed)
					.await
			})
			.await
			.unwrap();
		let dead = store
			.read(async |tx| tx.pairing_offer().await)
			.await
			.unwrap();

		assert_eq!(
			dead,
			Some(recorded(
				&offered,
				PairingOfferState::Invalidated {
					reason: PairingInvalidation::TooManyAttempts,
				},
				None
			))
		);
	}
}
