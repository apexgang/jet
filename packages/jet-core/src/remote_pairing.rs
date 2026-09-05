//! Restricted enrollment material, available only for a successful claim.

use crate::{
	ClientId, ClientPublicKey, Core, CoreError, PairingChallenge,
	PairingOfferId,
};

impl Core {
	/// Returns the signing material for this installation's current claim.
	/// The transport calls this only after its one-time secret was accepted.
	///
	/// # Errors
	/// Refuses a missing, different, or expired claim without exposing Plane state.
	pub async fn remote_pairing_signing_bytes(
		&self,
		client_id: ClientId,
		offer_id: PairingOfferId,
	) -> Result<Vec<u8>, CoreError> {
		self.store
			.read(async |tx| {
				let record = tx
					.pairing_offer()
					.await?
					.ok_or_else(crate::remote::unauthorized)?;
				if record.offer_id != offer_id.0
					|| record.expires_at_unix_ms <= self.now_unix_ms()
				{
					return Err(crate::remote::unauthorized());
				}
				let claim = record
					.claim
					.filter(|claim| claim.client_id == client_id.0)
					.ok_or_else(crate::remote::unauthorized)?;
				Ok(crate::pairing_secret::transcript(
					tx.plane().await?.plane_id,
					record.offer_id,
					client_id.0,
					&ClientPublicKey {
						algorithm: claim.key_algorithm,
						key: claim.public_key,
					},
					&PairingChallenge(claim.challenge),
				))
			})
			.await
	}
}
