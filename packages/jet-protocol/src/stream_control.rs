//! Strict JSON control carried inside one numbered binary stream.

use serde::{Deserialize, Serialize};

use crate::artifact::Sha256Digest;

/// Stream-local control carried as strict JSON on a numbered stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamControl {
	/// Receiver permits this many additional raw bytes.
	Credit {
		/// Additional byte credit.
		bytes: u64,
	},
	/// Rolling terminal output omitted a contiguous byte range.
	TerminalGap {
		/// Offset of the first omitted byte.
		#[serde(with = "crate::decimal")]
		first_missing_offset: u64,
		/// Number of omitted bytes.
		#[serde(with = "crate::decimal")]
		missing_bytes: u64,
	},
	/// A terminal stream ended at this source offset.
	TerminalFinished {
		/// Total source bytes, including any explicitly reported gaps.
		#[serde(with = "crate::decimal")]
		total_bytes: u64,
	},
	/// An Artifact stream ended with a declared size and integrity digest.
	ArtifactFinished {
		/// Total bytes in the completed Artifact.
		#[serde(with = "crate::decimal")]
		total_bytes: u64,
		/// SHA-256 digest computed across the complete content.
		sha256: Sha256Digest,
	},
}
