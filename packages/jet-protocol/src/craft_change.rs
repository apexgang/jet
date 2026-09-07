//! Craft 1.3 content evidence; the host assigns the Harness origin.
use serde::{Deserialize, Serialize};
/// A native file-operation receipt. Timing alone is never a receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CraftFileChange {
	/// Native operation identity retained with the source record.
	pub activity_id: String,
	/// Repository-relative path.
	pub path: String,
	/// Original Git blob or gitlink object, all zeros for additions.
	pub before_object: String,
	/// Resulting Git blob or gitlink object, all zeros for deletions.
	pub after_object: String,
	/// Original Git mode.
	pub before_mode: String,
	/// Resulting Git mode.
	pub after_mode: String,
}
