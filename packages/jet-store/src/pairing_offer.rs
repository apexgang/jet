//! The one Pairing offer a Plane may have open, and the claim a GUI client
//! makes against it (ADR-0017).
//!
//! A Plane pairs with one client at a time, so this is a single row:
//! opening an offer replaces whatever was open. No column can hold the
//! offer's one-time secret. What is stored is a salted digest of it, so a
//! store that is read cannot be used to claim the offer it describes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StoreError;
use crate::records::{column_error, parse_optional_uuid, parse_uuid};
use crate::transaction::{ReadTransaction, WriteTransaction};

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
#[serde(rename_all = "snake_case")]
pub enum PairingOfferState {
	/// Issued, and waiting for a client to present its secret.
	Offered,
	/// Claimed by a client. Both sides now show the authentication string,
	/// and the pairing waits for the people at each end.
	AwaitingConfirmation,
	/// Dead. It cannot be claimed or confirmed, only replaced.
	Invalidated,
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
	/// How far it has got.
	pub state: PairingOfferState,
	/// Why it stopped being usable, once it has.
	pub invalidation: Option<PairingInvalidation>,
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
	fn as_str(self) -> &'static str {
		match self {
			Self::Offered => "offered",
			Self::AwaitingConfirmation => "awaiting_confirmation",
			Self::Invalidated => "invalidated",
		}
	}

	fn parse(state: &str) -> Result<Self, StoreError> {
		match state {
			"offered" => Ok(Self::Offered),
			"awaiting_confirmation" => Ok(Self::AwaitingConfirmation),
			"invalidated" => Ok(Self::Invalidated),
			state => Err(column_error(
				"state",
				format!("unknown Pairing offer state {state:?}"),
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

	fn parse(algorithm: &str) -> Result<Self, StoreError> {
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
				key_algorithm, public_key, challenge, authentication_string
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
				public_key: parse_key("public_key", public_key)?,
				challenge: parse_key("challenge", challenge)?,
				authentication_string,
			}),
			_ => None,
		};
		Ok(Some(PairingOfferRecord {
			offer_id: parse_uuid("offer_id", &row.offer_id)?,
			method: PairingMethod::parse(&row.method, row.endpoint)?,
			secret_salt: parse_salt(row.secret_salt)?,
			secret_digest: parse_key("secret_digest", row.secret_digest)?,
			state: PairingOfferState::parse(&row.state)?,
			invalidation: row
				.invalidation
				.as_deref()
				.map(PairingInvalidation::parse)
				.transpose()?,
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
		let state = PairingOfferState::Offered.as_str();
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
			invalidation: None,
			failed_attempts: 0,
			opened_by,
			opened_at_unix_ms,
			expires_at_unix_ms,
			claim: None,
		})
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
		let state = PairingOfferState::AwaitingConfirmation.as_str();
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
		invalidation: PairingInvalidation,
	) -> Result<(), StoreError> {
		let state = PairingOfferState::Invalidated.as_str();
		let invalidation = invalidation.as_str();
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

fn parse_salt(bytes: Vec<u8>) -> Result<[u8; 16], StoreError> {
	let length = bytes.len();
	bytes.try_into().map_err(|_| {
		column_error("secret_salt", format!("the salt has {length} bytes"))
	})
}

fn parse_key(column: &str, bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
	let length = bytes.len();
	bytes.try_into().map_err(|_| {
		column_error(column, format!("the value has {length} bytes"))
	})
}

#[cfg(test)]
#[path = "pairing_offer_tests.rs"]
mod tests;
