//! Links the shared error type without Recovery cryptography for the size gate.
#[path = "support/recovery_error.rs"]
mod error;
use error::CoreError;
fn main() -> Result<(), CoreError> {
	if std::env::args().len() > 1 {
		return Err(CoreError::invalid_input(
			"recovery.invalid_bundle",
			"size probe",
		));
	}
	Ok(())
}
