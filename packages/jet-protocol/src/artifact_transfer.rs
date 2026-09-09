//! Lossless Artifact operations carried on numbered binary streams.
use crate::Sha256Digest;
use serde::{Deserialize, Serialize};

/// Immutable Artifact declaration, sent before content bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArtifactDescriptor {
	/// Content address verified at publication and download completion.
	pub sha256: Sha256Digest,
	/// Exact byte count.
	#[serde(with = "crate::decimal")]
	#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
	pub size: u64,
}

/// Artifact stream requests and replies, available from minor 31.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactControl {
	/// Begin a private upload. The server responds with byte credit.
	ArtifactUpload {
		/// Run that will own the committed reference.
		run_id: uuid::Uuid,
		/// Declaration that all incoming bytes must satisfy.
		artifact: ArtifactDescriptor,
	},
	/// Commit a completely received and verified upload.
	ArtifactCommit,
	/// Successful durable publication. Retrying a Run/hash is idempotent.
	ArtifactPublished {
		/// Verified content identity.
		artifact: ArtifactDescriptor,
	},
	/// Request published content. Grant Credit after receiving its declaration.
	ArtifactDownload {
		/// Content address to retrieve.
		sha256: Sha256Digest,
	},
	/// Declaration preceding a download's data and ArtifactFinished control.
	ArtifactDownloading {
		/// Exact size and integrity contract.
		artifact: ArtifactDescriptor,
	},
	/// Discard staging or stop delivery. A previously requested commit may finish.
	ArtifactCancel,
	/// Delivery stopped. A previously requested commit may still become durable.
	ArtifactCanceled,
	/// Collect abandoned payloads past their 24-hour grace period.
	ArtifactCollect,
	/// Collection finished, including an explicit no-op result.
	ArtifactCollected {
		/// Number of unreferenced files removed in this bounded batch.
		removed: u32,
	},
}
