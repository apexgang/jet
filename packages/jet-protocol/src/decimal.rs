//! Decimal-string encoding of sequences and revisions (ADR-0089).
//!
//! JavaScript and Swift JSON decoders lose precision above 2^53, so every
//! Plane sequence, journal cursor, and Revision crosses the wire as a
//! decimal string such as `"42"`. Readers accept only canonical digits:
//! no sign, no leading zeros, nothing a lenient parser would have to guess
//! about (ADR-0094).

use serde::de::{Error, Unexpected};
use serde::{Deserialize, Deserializer, Serializer};

#[expect(
	clippy::trivially_copy_pass_by_ref,
	reason = "serde's serialize_with contract passes the field by reference"
)]
pub(crate) fn serialize<S: Serializer>(
	value: &u64,
	serializer: S,
) -> Result<S::Ok, S::Error> {
	serializer.collect_str(value)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
	deserializer: D,
) -> Result<u64, D::Error> {
	let text = String::deserialize(deserializer)?;
	parse(&text).ok_or_else(|| {
		D::Error::invalid_value(
			Unexpected::Str(&text),
			&"a canonical decimal string",
		)
	})
}

fn parse(text: &str) -> Option<u64> {
	let canonical = !text.is_empty()
		&& text.bytes().all(|byte| byte.is_ascii_digit())
		&& (text == "0" || !text.starts_with('0'));
	canonical.then(|| text.parse().ok()).flatten()
}

pub(crate) mod optional {
	use serde::{Deserialize, Deserializer, Serializer};

	pub(crate) fn serialize<S: Serializer>(
		value: &Option<u64>,
		serializer: S,
	) -> Result<S::Ok, S::Error> {
		match value {
			Some(value) => serializer.collect_str(value),
			None => serializer.serialize_none(),
		}
	}

	pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Option<u64>, D::Error> {
		let Some(text) = Option::<String>::deserialize(deserializer)? else {
			return Ok(None);
		};
		super::parse(&text).map(Some).ok_or_else(|| {
			serde::de::Error::invalid_value(
				serde::de::Unexpected::Str(&text),
				&"a canonical decimal string",
			)
		})
	}
}
