//! The pairing transcript a remote Plane hands back on a claim, and the
//! authentication string both screens show (ADR-0017).
//!
//! The layout and the string are copied from `jet-core`'s `pub(crate)`
//! helpers (`pairing/secret.rs`) and the client contract in
//! `jet-daemon/tests/support/pairing.rs`, because `jet-protocol` does not
//! export them yet (backend dependency `pairing_transcript_helpers`). Drift
//! fails closed as `enrollment.transcript_invalid`.
//!
//! Validation keeps a hostile endpoint from getting an arbitrary message
//! signed: only a transcript that names the claimed offer, this installation
//! and this installation's own key is ever passed to `sign_pairing`.
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::errors::PublicError;

const TRANSCRIPT_DOMAIN: &[u8] = b"jet.pairing.transcript.v1";
const AUTHENTICATION_DOMAIN: &[u8] = b"jet.pairing.authentication.v1";
const ALGORITHM: &[u8] = b"ed25519";
const SEPARATOR: u8 = 0;
/// domain ‖ 0 ‖ plane[16] ‖ offer[16] ‖ client[16] ‖ "ed25519" ‖ 0 ‖ key[32] ‖ challenge[32]
pub(crate) const TRANSCRIPT_LENGTH: usize =
    TRANSCRIPT_DOMAIN.len() + 1 + 16 * 3 + ALGORITHM.len() + 1 + 32 + 32;

/// A transcript that names the claimed offer, this installation and its own
/// key. Nothing else can be signed as a pairing proof.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ValidatedTranscript {
    bytes: [u8; TRANSCRIPT_LENGTH],
    plane_identity: Uuid,
}

impl std::fmt::Debug for ValidatedTranscript {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedTranscript")
            .field("plane_identity", &self.plane_identity)
            .finish_non_exhaustive()
    }
}

impl ValidatedTranscript {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Whether the transcript names `key` as the key being paired.
    pub(crate) fn names_key(&self, key: &[u8; 32]) -> bool {
        let offset = TRANSCRIPT_DOMAIN.len() + 1 + 16 * 3 + ALGORITHM.len() + 1;
        &self.bytes[offset..offset + 32] == key
    }

    /// The Plane identity the target claims, bound into what both screens
    /// compare.
    pub(crate) fn plane_identity(&self) -> Uuid {
        self.plane_identity
    }
}

pub(crate) fn transcript_invalid() -> PublicError {
    PublicError::invalid_response_code(
        "enrollment.transcript_invalid",
        "The Plane's pairing reply didn't check out, so nothing was paired.",
    )
}

pub(crate) fn identity_changed() -> PublicError {
    PublicError::conflict(
        "plane.identity_changed",
        "This SSH address now reaches a different Plane.",
    )
}

fn uuid_at(bytes: &[u8], offset: usize) -> Uuid {
    let mut value = [0u8; 16];
    value.copy_from_slice(&bytes[offset..offset + 16]);
    Uuid::from_bytes(value)
}

/// Checks a claim's `signing_bytes` against what this computer asked for.
/// `repair_of` is the stored identity of a Plane being paired again.
pub(crate) fn validate(
    bytes: &[u8],
    expected_offer: Uuid,
    own_client: Uuid,
    own_key: &[u8; 32],
    repair_of: Option<Uuid>,
) -> Result<ValidatedTranscript, PublicError> {
    let bytes: [u8; TRANSCRIPT_LENGTH] = bytes.try_into().map_err(|_| transcript_invalid())?;
    let mut offset = 0;
    if &bytes[..TRANSCRIPT_DOMAIN.len()] != TRANSCRIPT_DOMAIN
        || bytes[TRANSCRIPT_DOMAIN.len()] != SEPARATOR
    {
        return Err(transcript_invalid());
    }
    offset += TRANSCRIPT_DOMAIN.len() + 1;
    let plane_identity = uuid_at(&bytes, offset);
    offset += 16;
    let offer = uuid_at(&bytes, offset);
    offset += 16;
    let client = uuid_at(&bytes, offset);
    offset += 16;
    let algorithm_ok = &bytes[offset..offset + ALGORITHM.len()] == ALGORITHM
        && bytes[offset + ALGORITHM.len()] == SEPARATOR;
    offset += ALGORITHM.len() + 1;
    let key_ok = &bytes[offset..offset + 32] == own_key;
    if !algorithm_ok || offer != expected_offer || client != own_client || !key_ok {
        return Err(transcript_invalid());
    }
    if repair_of.is_some_and(|stored| stored != plane_identity) {
        return Err(identity_changed());
    }
    Ok(ValidatedTranscript {
        bytes,
        plane_identity,
    })
}

/// SHA-256(domain ‖ 0 ‖ transcript), first 8 bytes big-endian mod 10⁶,
/// formatted `ddd-ddd`. The shell shows this string, never the server's.
pub(crate) fn authentication_string(transcript: &ValidatedTranscript) -> String {
    let mut hash = Sha256::new();
    hash.update(AUTHENTICATION_DOMAIN);
    hash.update([SEPARATOR]);
    hash.update(transcript.bytes());
    let digest = hash.finalize();
    let mut drawn = [0u8; 8];
    drawn.copy_from_slice(&digest[..8]);
    let value = u64::from_be_bytes(drawn) % 1_000_000;
    let digits = format!("{value:06}");
    format!("{}-{}", &digits[..3], &digits[3..])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const PLANE: Uuid = Uuid::from_u128(0x1111);
    pub(crate) const OFFER: Uuid = Uuid::from_u128(0x2222);
    pub(crate) const CLIENT: Uuid = Uuid::from_u128(0x3333);
    const KEY: [u8; 32] = [0x44; 32];

    /// Built from the documented layout, the way `jet-daemon`'s client
    /// contract builds it.
    pub(crate) fn golden(plane: Uuid, offer: Uuid, client: Uuid, key: &[u8; 32]) -> Vec<u8> {
        let mut transcript = Vec::new();
        transcript.extend_from_slice(b"jet.pairing.transcript.v1");
        transcript.push(0);
        transcript.extend_from_slice(plane.as_bytes());
        transcript.extend_from_slice(offer.as_bytes());
        transcript.extend_from_slice(client.as_bytes());
        transcript.extend_from_slice(b"ed25519");
        transcript.push(0);
        transcript.extend_from_slice(key);
        transcript.extend_from_slice(&[0x55; 32]);
        transcript
    }

    #[test]
    fn a_golden_transcript_validates() {
        let bytes = golden(PLANE, OFFER, CLIENT, &KEY);
        assert_eq!(bytes.len(), TRANSCRIPT_LENGTH);
        let validated = validate(&bytes, OFFER, CLIENT, &KEY, None).unwrap();
        assert_eq!(validated.plane_identity(), PLANE);
        assert_eq!(validated.bytes(), &bytes[..]);
        assert!(validated.names_key(&KEY));
        assert!(!validated.names_key(&[0x45; 32]));
        assert!(validate(&bytes, OFFER, CLIENT, &KEY, Some(PLANE)).is_ok());
    }

    #[test]
    fn every_field_mutation_is_rejected() {
        let bytes = golden(PLANE, OFFER, CLIENT, &KEY);
        let invalid = |bytes: &[u8]| {
            validate(bytes, OFFER, CLIENT, &KEY, None)
                .err()
                .map(|error| error.code)
        };
        let mut domain = bytes.clone();
        domain[0] ^= 1;
        assert_eq!(
            invalid(&domain).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        assert_eq!(
            invalid(&bytes[..bytes.len() - 1]).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(
            invalid(&longer).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        let other_offer = golden(PLANE, Uuid::from_u128(9), CLIENT, &KEY);
        assert_eq!(
            invalid(&other_offer).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        let other_client = golden(PLANE, OFFER, Uuid::from_u128(9), &KEY);
        assert_eq!(
            invalid(&other_client).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        let other_key = golden(PLANE, OFFER, CLIENT, &[0x45; 32]);
        assert_eq!(
            invalid(&other_key).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        let mut algorithm = bytes.clone();
        algorithm[26 + 48] = b'E';
        assert_eq!(
            invalid(&algorithm).as_deref(),
            Some("enrollment.transcript_invalid")
        );
        let error = transcript_invalid();
        assert_eq!(error.category, "invalid_response");
        assert!(!error.retryable);
    }

    #[test]
    fn a_repair_that_reaches_another_plane_is_an_identity_change() {
        let bytes = golden(Uuid::from_u128(0x9999), OFFER, CLIENT, &KEY);
        let error = validate(&bytes, OFFER, CLIENT, &KEY, Some(PLANE)).unwrap_err();
        assert_eq!(error.code, "plane.identity_changed");
        assert_eq!(error.category, "conflict");
    }

    #[test]
    fn the_authentication_string_is_deterministic_and_grouped() {
        let first = validate(
            &golden(PLANE, OFFER, CLIENT, &KEY),
            OFFER,
            CLIENT,
            &KEY,
            None,
        )
        .unwrap();
        let again = validate(
            &golden(PLANE, OFFER, CLIENT, &KEY),
            OFFER,
            CLIENT,
            &KEY,
            None,
        )
        .unwrap();
        let string = authentication_string(&first);
        assert_eq!(string, authentication_string(&again));
        let bytes = string.as_bytes();
        assert_eq!(bytes.len(), 7);
        assert_eq!(bytes[3], b'-');
        assert!(bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 3 || byte.is_ascii_digit()));
        // The same computation, spelled out: SHA-256 over domain ‖ 0 ‖ t.
        let mut hash = Sha256::new();
        hash.update(b"jet.pairing.authentication.v1\0");
        hash.update(first.bytes());
        let digest = hash.finalize();
        let value = u64::from_be_bytes(digest[..8].try_into().unwrap()) % 1_000_000;
        assert_eq!(string.replace('-', ""), format!("{value:06}"));
        // Any change to the transcript is a different string.
        let other = validate(
            &golden(Uuid::from_u128(0x1112), OFFER, CLIENT, &KEY),
            OFFER,
            CLIENT,
            &KEY,
            None,
        )
        .unwrap();
        assert_ne!(authentication_string(&other), string);
    }
}
