//! Facts a trusted Run Adapter can submit for one managed execution.

use crate::{
	ApprovalRequest, ChangeEvidence, CoreError, RunActivity, TurnOutcome,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SourcePrefix {
	pub(crate) count: usize,
	pub(crate) digest: String,
}

impl SourcePrefix {
	pub(crate) fn include(
		&mut self,
		observation: &Observation,
	) -> Result<(), CoreError> {
		use sha2::{Digest, Sha256};
		let bytes = serde_json::to_vec(observation).map_err(|_| invalid())?;
		let mut hash = Sha256::new();
		hash.update(self.digest.as_bytes());
		hash.update(bytes);
		self.digest = format!("{:x}", hash.finalize());
		self.count += 1;
		Ok(())
	}
}

pub(crate) enum SourceBoundary {
	Pending,
	Complete { offset: u64, checkpoint: String },
}

/// Facts from the trusted Run Adapter, validated against the durable lifecycle.
#[derive(Serialize)]
pub enum Observation {
	/// Explicit Model selected by the native session, distinct from consumption.
	Model(crate::ModelId),
	/// Native content receipt; the Run Adapter assigns Harness origin.
	FileChanged(ChangeEvidence),
	/// The trusted Adapter has held a new turn's input pending durable capture.
	/// It may release that input only after acknowledging this source boundary.
	TurnStarted,
	/// A turn ended while the Run may remain active.
	TurnEnded(TurnOutcome),
	/// Legacy Craft Command completion, including native Conversation identity.
	Completed(String),
	/// Explicit completion of one admitted input, independent of Run activity.
	TurnCompleted {
		/// Correlation identity originally delivered to the pinned Craft.
		turn_id: uuid::Uuid,
		/// Harness-native Conversation identity for later continuation.
		native_conversation: String,
	},
	/// The helper reported that it spawned a Harness.
	Started {
		/// Actual helper OS identity.
		helper_pid: u32,
		/// Native OS identity supplied by the helper.
		harness_pid: u32,
	},
	/// A structured native title candidate for the owning Conversation.
	ConversationTitle(String),
	/// A structured native title candidate for this Run.
	RunTitle(String),
	/// A terminal/native title for exactly one Managed process.
	ProcessTitle {
		/// Existing process identity in this execution.
		pid: u32,
		/// Unescaped title text.
		title: String,
	},
	/// An active Harness began working or waiting.
	Activity(RunActivity),
	/// What the Harness reported about its own consumption, or about a
	/// Provider quota window, normalized by its Craft (ADR-0023).
	Usage(crate::UsageReport),
	/// The Harness asked for something that needs a decision and is
	/// waiting for exactly one. The Craft keeps holding the native
	/// request until Core answers it.
	ApprovalRequested(ApprovalRequest),
	/// Lossless native JSON and its portable views.
	Output {
		/// Original native JSON bytes.
		native_json: String,
		/// Portable Presentation blocks, preserving unknown data.
		presentation_json: Vec<String>,
	},
	/// Native identity for a later explicit resume.
	NativeConversation(String),
	/// End offset of source whose observations preceded this marker.
	Progress {
		/// End of the source batch.
		offset: u64,
		/// Adapter parser state at that boundary.
		checkpoint: String,
	},
	/// Reaped native exit status, absent for signal termination.
	Ended(Option<i32>),
	/// Definite launch rejection with no surviving Harness.
	LaunchFailed,
	/// The supervising connection was lost.
	Disconnected,
	/// A validated Craft has reattached to the original helper.
	Reconnected,
	/// The previous execution is proven gone; later work requires a new Run.
	Lost,
}

fn invalid() -> CoreError {
	CoreError::conflict(
		"run.invalid_observation",
		"the Craft observation conflicts with the Run lifecycle",
	)
}
