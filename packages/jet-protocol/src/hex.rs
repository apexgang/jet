//! Fixed-width byte strings on the wire, as lowercase hexadecimal.
//!
//! JSON has no byte string, and a control frame is strict JSON (ADR-0089),
//! so keys, challenges, and signatures cross as hexadecimal of exactly the
//! width their algorithm fixes. Anything else fails to decode, which keeps
//! a malformed key a protocol error rather than something the Plane has to
//! decide about later.

use serde::{Deserialize, Deserializer, Serializer};

pub(crate) fn serialize<const N: usize, S: Serializer>(
	bytes: &[u8; N],
	serializer: S,
) -> Result<S::Ok, S::Error> {
	let mut text = String::with_capacity(N * 2);
	for byte in bytes {
		use std::fmt::Write as _;
		let _ = write!(text, "{byte:02x}");
	}
	serializer.serialize_str(&text)
}

pub(crate) fn deserialize<'de, const N: usize, D: Deserializer<'de>>(
	deserializer: D,
) -> Result<[u8; N], D::Error> {
	let text = <&str>::deserialize(deserializer)?;
	if text.len() != N * 2 {
		return Err(serde::de::Error::custom(format!(
			"expected {} hexadecimal characters, got {}",
			N * 2,
			text.len()
		)));
	}
	let mut bytes = [0u8; N];
	let (pairs, remainder) = text.as_bytes().as_chunks::<2>();
	debug_assert!(remainder.is_empty());
	for (index, pair) in pairs.iter().enumerate() {
		let (high, low) = (nibble(pair[0]), nibble(pair[1]));
		let (Some(high), Some(low)) = (high, low) else {
			return Err(serde::de::Error::custom(
				"expected lowercase hexadecimal",
			));
		};
		bytes[index] = (high << 4) | low;
	}
	Ok(bytes)
}

/// One lowercase hexadecimal digit. Uppercase is refused, so one value has
/// one encoding and a digest of the frame means what it says.
fn nibble(character: u8) -> Option<u8> {
	match character {
		b'0'..=b'9' => Some(character - b'0'),
		b'a'..=b'f' => Some(character - b'a' + 10),
		_ => None,
	}
}
