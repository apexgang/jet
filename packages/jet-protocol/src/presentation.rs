//! Portable, non-executable presentation with lossless opaque fallback.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use crate::{ControlError, decode_control};

/// Presentation understood by every GUI; never executable UI code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Presentation {
	/// Plain text rendered without markup interpretation.
	Text {
		/// Human-visible text.
		text: String,
	},
	/// Markdown rendered with the GUI's normal sanitization rules.
	Markdown {
		/// Human-visible Markdown.
		text: String,
	},
	/// Named native actions routed back through authenticated Jet Commands.
	Actions {
		/// Choices available for this native event.
		actions: Vec<PresentationAction>,
	},
}

/// An action description grants no broker permission or approval authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PresentationAction {
	/// Harness-defined identity scoped to this execution.
	pub id: String,
	/// Plain-text user-facing label.
	pub label: String,
}

/// Original JSON retained even when this reader does not know the block.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct PresentationBlock(Box<RawValue>);

impl PresentationBlock {
	/// Builds a portable block without discarding its wire representation.
	///
	/// # Errors
	/// Returns a codec error if the resulting block exceeds protocol limits.
	pub fn new(presentation: &Presentation) -> Result<Self, ControlError> {
		decode_control(&crate::encode_control(presentation)?)
	}

	/// Original JSON, including unknown fields, for persistence and forwarding.
	#[must_use]
	pub fn raw(&self) -> &RawValue {
		&self.0
	}

	/// Returns a known view, or `None` for generic rendering of an opaque block.
	///
	/// # Errors
	/// Rejects malformed known blocks; they cannot bypass validation as opaque.
	pub fn known(&self) -> Result<Option<Presentation>, ControlError> {
		#[derive(Deserialize)]
		struct Kind {
			kind: String,
		}
		let kind: Kind = decode_control(self.0.get().as_bytes())?;
		match kind.kind.as_str() {
			"text" | "markdown" | "actions" => {
				decode_control(self.0.get().as_bytes()).map(Some)
			}
			_ => Ok(None),
		}
	}
}

impl<'de> Deserialize<'de> for PresentationBlock {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		let block = Self(Box::<RawValue>::deserialize(deserializer)?);
		// ASVS 1.5.2: only display blocks are extensible, never Commands.
		block.known().map_err(serde::de::Error::custom)?;
		Ok(block)
	}
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for PresentationBlock {
	fn schema_name() -> std::borrow::Cow<'static, str> {
		"PresentationBlock".into()
	}
	fn json_schema(
		generator: &mut schemars::SchemaGenerator,
	) -> schemars::Schema {
		let known = generator.subschema_for::<Presentation>();
		schemars::json_schema!({"anyOf": [known, {"type":"object", "required":["kind"], "properties":{"kind":{"type":"string", "not":{"enum":["text","markdown","actions"]}}}}]})
	}
}
