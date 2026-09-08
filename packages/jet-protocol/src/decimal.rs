//! Decimal-string encoding of sequences and revisions (ADR-0089).
//!
//! JavaScript and Swift JSON decoders lose precision above 2^53, so every
//! Plane sequence, journal cursor, and Revision crosses the wire as a
//! decimal string such as `"42"`. Readers accept only canonical digits:
//! no sign, no leading zeros, nothing a lenient parser would have to guess
//! about (ADR-0094).

use serde::de::{Error, Unexpected};
use serde::{Deserialize, Deserializer, Serializer};

/// The only spelling a reader accepts, and the one the schema publishes.
#[cfg(feature = "schema")]
pub(crate) const PATTERN: &str = "^(0|[1-9][0-9]*)$";

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

/// Schema stand-in for a sequence field. `serde(with)` is invisible to
/// schemars, which would otherwise publish the `u64` this codec hides.
#[cfg(feature = "schema")]
pub(crate) struct Decimal;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Decimal {
	fn schema_name() -> std::borrow::Cow<'static, str> {
		"Decimal".into()
	}

	fn inline_schema() -> bool {
		true
	}

	fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
		schemars::json_schema!({"type": "string", "pattern": PATTERN})
	}
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

	/// Schema stand-in for a field this codec may leave absent.
	#[cfg(feature = "schema")]
	pub(crate) struct OptionalDecimal;

	#[cfg(feature = "schema")]
	impl schemars::JsonSchema for OptionalDecimal {
		fn schema_name() -> std::borrow::Cow<'static, str> {
			"OptionalDecimal".into()
		}

		fn inline_schema() -> bool {
			true
		}

		fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
			schemars::json_schema!({
				"type": ["string", "null"],
				"pattern": super::PATTERN,
			})
		}
	}
}
