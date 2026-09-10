//! Conversation and Run naming authority (ADR-0044).
//!
//! This module owns validation and source precedence. Storage creates and
//! preserves deterministic fallback text. Callers submit a complete [`Name`];
//! they never reimplement which automatic source may replace another one.

use crate::{
	Actor, CommandOutcome, ConflictState, Conversation, ConversationId,
	CoreError, EventActor, EventKind, Revision, RevisionConflict, Run, RunId,
	event::EventSubject,
};
use jet_store::{NameRecord, NameSourceRecord};
use serde::{Deserialize, Serialize};

/// Most UTF-8 bytes retained for a user-facing name or native process label.
pub const MAX_NAME_BYTES: usize = 256;

/// Authority that supplied a current Conversation or Run name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
	/// An interactive user's choice. No automatic source may replace it.
	Manual,
	/// A validated Utility-model naming result. Utility result attribution is
	/// recorded by the Utility module that executes the work (ADR-0099).
	Utility,
	/// A structured native title supplied by the Harness through its Craft.
	HarnessNative,
	/// Stable local text available without Utility work.
	Deterministic,
}

impl NameSource {
	fn precedence(self) -> u8 {
		match self {
			Self::Manual => 4,
			Self::Utility => 3,
			Self::HarnessNative => 2,
			Self::Deterministic => 1,
		}
	}
}

/// One resolved, unescaped user-facing name and the authority that supplied it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Name {
	/// Original validated text; each rendering context encodes it at output.
	pub value: String,
	/// Authority that supplied `value`.
	pub source: NameSource,
}

impl Name {
	/// Validates one authoritative manual name at the trusted core seam.
	///
	/// # Errors
	///
	/// Returns `name.invalid` when the text is empty, padded, oversized, or
	/// contains control characters.
	pub fn manual(value: impl Into<String>) -> Result<Self, CoreError> {
		Self::validated(value.into(), NameSource::Manual)
	}

	pub(crate) fn harness_native(
		value: impl Into<String>,
	) -> Result<Self, CoreError> {
		Self::validated(value.into(), NameSource::HarnessNative)
	}

	/// Whether this candidate has enough authority to replace `current`.
	/// Manual names may be edited manually. Automatic candidates may refresh a
	/// name at the same authority or move it upward through the ADR-0044 order,
	/// but may never replace a manual name.
	pub(crate) fn replaces(&self, current: &Self) -> bool {
		self.source == NameSource::Manual
			|| (current.source != NameSource::Manual
				&& self.source.precedence() >= current.source.precedence())
	}

	pub(crate) fn process_label(
		value: impl Into<String>,
	) -> Result<String, CoreError> {
		Self::validated(value.into(), NameSource::HarnessNative)
			.map(|name| name.value)
	}

	fn validated(value: String, source: NameSource) -> Result<Self, CoreError> {
		// ASVS 2.2.1 and 2.2.2: enforce the documented positive shape at the
		// trusted core, including bounded allocation for untrusted Utility or
		// Craft output. Encoding remains a rendering concern (ASVS 1.1.2).
		if value.is_empty()
			|| value.len() > MAX_NAME_BYTES
			|| value.trim() != value
			|| value.chars().any(char::is_control)
		{
			return Err(CoreError::invalid_input(
				"name.invalid",
				"a name must be 1 to 256 UTF-8 bytes with no surrounding whitespace or control characters",
			));
		}
		Ok(Self { value, source })
	}
}

pub(crate) async fn apply_harness_conversation(
	tx: &mut jet_store::WriteTransaction,
	actor: &EventActor,
	conversation_id: ConversationId,
	originating_run_id: RunId,
	value: String,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let candidate = Name::harness_native(value)?;
	let Some(existing) = tx.conversation(conversation_id.0).await? else {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	};
	if !candidate.replaces(&existing.name.into()) {
		return Ok(());
	}
	let record = NameRecord::from(candidate.clone());
	tx.update_conversation_name(conversation_id.0, &record)
		.await?;
	tx.append_event(
		EventKind::ConversationNameChanged { name: candidate }.to_record_as(
			actor.clone(),
			EventSubject::Run {
				conversation_id,
				run_id: originating_run_id,
			},
			now_unix_ms,
		)?,
	)
	.await?;
	Ok(())
}

pub(crate) async fn apply_harness_run(
	tx: &mut jet_store::WriteTransaction,
	actor: &EventActor,
	run_id: RunId,
	value: String,
	now_unix_ms: i64,
) -> Result<(), CoreError> {
	let candidate = Name::harness_native(value)?;
	let Some(existing) = tx.run(run_id.0).await? else {
		return Err(CoreError::not_found(
			"run.not_found",
			"the Run does not exist",
		));
	};
	if !candidate.replaces(&existing.name.into()) {
		return Ok(());
	}
	let conversation_id = ConversationId(existing.conversation_id);
	let record = NameRecord::from(candidate.clone());
	tx.update_run_name(run_id.0, &record).await?;
	tx.append_event(
		EventKind::RunNameChanged { name: candidate }.to_record_as(
			actor.clone(),
			EventSubject::Run {
				conversation_id,
				run_id,
			},
			now_unix_ms,
		)?,
	)
	.await?;
	Ok(())
}

impl From<NameRecord> for Name {
	fn from(record: NameRecord) -> Self {
		Self {
			value: record.value,
			source: match record.source {
				NameSourceRecord::Manual => NameSource::Manual,
				NameSourceRecord::Utility => NameSource::Utility,
				NameSourceRecord::HarnessNative => NameSource::HarnessNative,
				NameSourceRecord::Deterministic => NameSource::Deterministic,
			},
		}
	}
}

impl From<Name> for NameRecord {
	fn from(name: Name) -> Self {
		Self {
			value: name.value,
			source: match name.source {
				NameSource::Manual => NameSourceRecord::Manual,
				NameSource::Utility => NameSourceRecord::Utility,
				NameSource::HarnessNative => NameSourceRecord::HarnessNative,
				NameSource::Deterministic => NameSourceRecord::Deterministic,
			},
		}
	}
}

pub(crate) async fn set_conversation(
	tx: &mut jet_store::WriteTransaction,
	actor: &Actor,
	conversation_id: ConversationId,
	expected_revision: Revision,
	name: Name,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	// This core API is itself a trust boundary: callers cannot smuggle an
	// automatic source or bypass text validation by constructing `Name`.
	let name = Name::manual(name.value)?;
	let Some(existing) = tx.conversation(conversation_id.0).await? else {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	};
	if existing.revision != expected_revision.0 {
		let current: Conversation = existing.into();
		return Err(CoreError::revision_conflict(
			"conversation.revision_conflict",
			"the Conversation changed since the Command was prepared",
			RevisionConflict {
				current_revision: current.revision,
				safe_state: ConflictState::Conversation(current),
			},
		));
	}
	let record = NameRecord::from(name.clone());
	// ASVS 2.3.3: current state and its Event commit in one transaction.
	tx.update_conversation_name(conversation_id.0, &record)
		.await?;
	tx.append_event(EventKind::ConversationNameChanged { name }.to_record(
		actor,
		EventSubject::Conversation(conversation_id),
		now_unix_ms,
	)?)
	.await?;
	let conversation: Conversation = tx
		.conversation(conversation_id.0)
		.await?
		.ok_or_else(missing_conversation)?
		.into();
	Ok(CommandOutcome::ConversationNamed(conversation))
}

pub(crate) async fn set_run(
	tx: &mut jet_store::WriteTransaction,
	actor: &Actor,
	run_id: RunId,
	expected_revision: Revision,
	name: Name,
	now_unix_ms: i64,
) -> Result<CommandOutcome, CoreError> {
	let name = Name::manual(name.value)?;
	let Some(existing) = tx.run(run_id.0).await? else {
		return Err(CoreError::not_found(
			"run.not_found",
			"the Run does not exist",
		));
	};
	let conversation_id = ConversationId(existing.conversation_id);
	if existing.revision != expected_revision.0 {
		let current: Run = existing.into();
		return Err(CoreError::revision_conflict(
			"run.revision_conflict",
			"the Run changed since the Command was prepared",
			RevisionConflict {
				current_revision: current.revision,
				safe_state: ConflictState::Run(current),
			},
		));
	}
	let record = NameRecord::from(name.clone());
	// ASVS 2.3.3: current state and its Event commit in one transaction.
	tx.update_run_name(run_id.0, &record).await?;
	tx.append_event(EventKind::RunNameChanged { name }.to_record(
		actor,
		EventSubject::Run {
			conversation_id,
			run_id,
		},
		now_unix_ms,
	)?)
	.await?;
	let run: Run = tx
		.run(run_id.0)
		.await?
		.ok_or_else(|| {
			CoreError::internal("run.missing", "the named Run disappeared")
		})?
		.into();
	Ok(CommandOutcome::RunNamed(run))
}

fn missing_conversation() -> CoreError {
	CoreError::internal(
		"conversation.missing",
		"the named Conversation disappeared",
	)
}
