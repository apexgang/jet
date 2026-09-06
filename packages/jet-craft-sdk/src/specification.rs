use crate::CraftError;
use jet_protocol::{CraftSpecification, decode_control, encode_control};

/// Parse bounded `.jet/craft-spec.toml` contents without accessing files.
///
/// # Errors
/// Rejects malformed, oversized, incompatible, or unknown required declarations.
pub fn parse_specification(
	text: &str,
) -> Result<CraftSpecification, CraftError> {
	// ASVS 2.2.1: cap the install-time document before allocating TOML values.
	if text.len() > 64 * 1024 {
		return Err(CraftError::InvalidMessage);
	}
	let spec: CraftSpecification =
		toml::from_str(text).map_err(|_| CraftError::InvalidMessage)?;
	let spec: CraftSpecification = decode_control(
		&encode_control(&spec).map_err(|_| CraftError::InvalidMessage)?,
	)
	.map_err(|_| CraftError::InvalidMessage)?;
	spec.enabled_features()
		.map_err(|_| CraftError::Incompatible)?;
	Ok(spec)
}
