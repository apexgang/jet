//! Strict core validation of untrusted model JSON. There is no Command decoder.
use crate::{
	CoreError, UtilityInput, UtilityOutcome, utility::work::unavailable,
};
use serde::Deserialize;

pub(crate) fn validate(
	input: &UtilityInput,
	bytes: &[u8],
) -> Result<UtilityOutcome, CoreError> {
	let invalid = || unavailable("utility.output_invalid");
	match input {
		UtilityInput::Autodelete { .. } => {
			#[derive(Deserialize)]
			#[serde(deny_unknown_fields)]
			struct Draft {
				inactive_days: u32,
			}
			let draft: Draft =
				serde_json::from_slice(bytes).map_err(|_| invalid())?;
			if !(1..=36500).contains(&draft.inactive_days) {
				return Err(invalid());
			}
			Ok(UtilityOutcome::Draft {
				inactive_days: draft.inactive_days,
			})
		}
		UtilityInput::GitText { .. } => {
			#[derive(Deserialize)]
			#[serde(deny_unknown_fields)]
			struct Text {
				subject: String,
				body: String,
			}
			let text: Text =
				serde_json::from_slice(bytes).map_err(|_| invalid())?;
			if text.subject.trim().is_empty()
				|| text.subject != text.subject.trim()
				|| text.subject.len() > 256
				|| text.subject.chars().any(char::is_control)
				|| text.body.len() > 4096
				|| text.body.chars().any(|c| c.is_control() && c != '\n')
			{
				return Err(invalid());
			}
			Ok(UtilityOutcome::Text {
				text: text.subject,
				body: text.body,
				fallback_reason: None,
			})
		}
		UtilityInput::Naming { .. } => {
			#[derive(Deserialize)]
			#[serde(deny_unknown_fields)]
			struct Name {
				text: String,
			}
			let name: Name =
				serde_json::from_slice(bytes).map_err(|_| invalid())?;
			let name = crate::Name::manual(name.text).map_err(|_| invalid())?;
			Ok(UtilityOutcome::Text {
				text: name.value,
				body: String::new(),
				fallback_reason: None,
			})
		}
	}
}
