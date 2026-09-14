//! The transfer bundle: what the source carries to the target, and the
//! checks the target makes before any of it reaches its store (ADR-0070).
//!
//! The bundle holds the Conversation, its historical Runs, its transcript
//! Events, its queued turns, its schedules, its Conversation-scoped
//! Settings, and its Artifact references with their payloads. Account
//! bindings, credentials, native Harness session files, execution plans,
//! and authorization Events stay behind, as they do in a portable
//! Recovery snapshot (ADR-0074).

use crate::{
	ArtifactDescriptor, CoreError, Name,
	conversation::name::MAX_NAME_BYTES,
	store_recovery::bundle::format::{self, Components},
};
use jet_store::{
	ActorRecord, PlaneTransferRecord, ReadTransaction, RetentionPolicy,
	RunLifecycle, SettingScopeRecord, TRANSCRIPT_EVENT_LIMIT,
	WorkingTreeRecord,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read as _};
use uuid::Uuid;

/// The bundle's own format version; a target that reads another refuses.
const FORMAT: u32 = 1;
/// Most Runs, schedules, Settings, and Artifacts one bundle carries.
const RUN_LIMIT: usize = 4096;
const SCHEDULE_LIMIT: usize = 32;
const SETTING_LIMIT: usize = 256;
const ARTIFACT_LIMIT: usize = 4096;
/// Most bytes one journal payload may hold, the journal's own bound.
const PAYLOAD_LIMIT: usize = 65_536;
/// Most bytes one queued-turn state or schedule may hold.
const STATE_LIMIT: usize = 1024 * 1024;
/// The transcript kinds a bundle carries.
const TRANSCRIPT_KINDS: [&str; 3] =
	["turn.input", "run.output", "artifact.published"];

/// Everything the target imports, as one JSON document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransferBundle {
	pub(crate) format: u32,
	pub(crate) transfer_id: Uuid,
	pub(crate) source_plane_id: Uuid,
	pub(crate) target_plane_id: Uuid,
	pub(crate) prepared_at_unix_ms: i64,
	pub(crate) conversation: BundledConversation,
	pub(crate) runs: Vec<BundledRun>,
	pub(crate) events: Vec<BundledEvent>,
	pub(crate) turn_queue: Option<String>,
	pub(crate) schedules: Vec<String>,
	pub(crate) settings: Vec<BundledSetting>,
	pub(crate) artifacts: Vec<BundledArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundledConversation {
	pub(crate) conversation_id: Uuid,
	pub(crate) retention: RetentionPolicy,
	pub(crate) name: Name,
	pub(crate) created_at_unix_ms: i64,
	/// The epoch the source retires; the target's copy is in the next.
	pub(crate) retired_epoch: u64,
	/// Whether the source worked in a Project, which the target must map
	/// onto one of its own.
	pub(crate) in_project: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundledRun {
	pub(crate) run_id: Uuid,
	pub(crate) lifecycle: RunLifecycle,
	pub(crate) name: Name,
	pub(crate) created_at_unix_ms: i64,
	pub(crate) ended_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundledEvent {
	pub(crate) event_id: Uuid,
	pub(crate) client_id: Uuid,
	pub(crate) recorded_at_unix_ms: i64,
	pub(crate) run_id: Option<Uuid>,
	pub(crate) kind: String,
	pub(crate) payload_version: u32,
	pub(crate) payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundledSetting {
	pub(crate) key: String,
	pub(crate) value: String,
	pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundledArtifact {
	pub(crate) run_id: Uuid,
	pub(crate) sha256: String,
	pub(crate) size: u64,
}

/// Reads everything the bundle carries from one consistent snapshot of
/// the source store. The Artifact payloads are read from disk afterwards.
pub(crate) async fn capture(
	tx: &mut ReadTransaction,
	transfer: &PlaneTransferRecord,
	source_plane_id: Uuid,
) -> Result<TransferBundle, CoreError> {
	let conversation_id = transfer.conversation_id;
	let conversation = tx
		.conversation(conversation_id)
		.await?
		.ok_or_else(super::conversation_not_found)?;
	let mut runs = vec![];
	let mut artifacts = vec![];
	for run in tx.runs(conversation_id).await? {
		for (sha256, size) in tx.run_artifacts(run.run_id).await? {
			artifacts.push(BundledArtifact {
				run_id: run.run_id,
				sha256,
				size: u64::try_from(size).unwrap_or_default(),
			});
		}
		runs.push(BundledRun {
			run_id: run.run_id,
			lifecycle: run.lifecycle,
			name: run.name.into(),
			created_at_unix_ms: run.created_at_unix_ms,
			ended_at_unix_ms: run.ended_at_unix_ms,
		});
	}
	let events = tx
		.transcript_events(conversation_id)
		.await?
		.into_iter()
		.map(|event| {
			let ActorRecord::InteractiveClient { client_id } = event.actor;
			BundledEvent {
				event_id: event.event_id,
				client_id,
				recorded_at_unix_ms: event.recorded_at_unix_ms,
				run_id: event.run_id,
				kind: event.kind,
				payload_version: event.payload_version,
				payload: event.payload,
			}
		})
		.collect();
	let settings = tx
		.settings_for_scope(SettingScopeRecord::Conversation {
			conversation_id,
		})
		.await?
		.into_iter()
		.map(|setting| BundledSetting {
			key: setting.key,
			value: setting.value,
			updated_at_unix_ms: setting.updated_at_unix_ms,
		})
		.collect();
	Ok(TransferBundle {
		format: FORMAT,
		transfer_id: transfer.transfer_id,
		source_plane_id,
		target_plane_id: transfer.peer_plane_id,
		prepared_at_unix_ms: transfer.prepared_at_unix_ms,
		conversation: BundledConversation {
			conversation_id,
			retention: conversation.retention,
			name: conversation.name.into(),
			created_at_unix_ms: conversation.created_at_unix_ms,
			retired_epoch: transfer.retired_epoch,
			in_project: match conversation.working_tree {
				WorkingTreeRecord::NoProject => false,
				WorkingTreeRecord::Workspace { .. }
				| WorkingTreeRecord::LocalCheckout { .. } => true,
			},
		},
		runs,
		events,
		turn_queue: tx.turn_queue(conversation_id).await?,
		schedules: tx.scheduled_tasks(conversation_id).await?,
		settings,
		artifacts,
	})
}

/// Encodes the bundle with every referenced payload read from `payloads`,
/// the Plane's Artifact directory, and verified against its reference.
/// Reading the same frozen Conversation twice yields the same bytes.
pub(crate) fn encode(
	bundle: &TransferBundle,
	payloads: &std::fs::File,
) -> Result<Vec<u8>, CoreError> {
	let mut parts = Components::new();
	let mut remaining = format::MAX_BYTES;
	parts.insert(
		"transfer.json".into(),
		serde_json::to_vec(bundle).map_err(|_| invalid_bundle())?,
	);
	for artifact in &bundle.artifacts {
		let name = format!("artifacts/{}", artifact.sha256);
		if parts.contains_key(&name) {
			continue;
		}
		let descriptor = ArtifactDescriptor {
			sha256: artifact.sha256.clone(),
			size: artifact.size,
		};
		let mut file =
			crate::artifact::files::open(payloads, &artifact.sha256)?;
		crate::artifact::files::verify(&mut file, &descriptor)?;
		let bytes = crate::store_recovery::bundle::read_bounded(
			file.by_ref(),
			&mut remaining,
		)?;
		parts.insert(name, bytes);
	}
	format::encode_in(&format::TRANSFER, parts).map_err(|_| invalid_bundle())
}

/// A bundle the target has read and checked, with its payloads by hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedBundle {
	pub(crate) bundle: TransferBundle,
	pub(crate) payloads: BTreeMap<String, Vec<u8>>,
}

/// Reads `bytes` back and checks everything the store would otherwise
/// have to trust: the format, the bounds, that every Event names a Run the
/// bundle carries with a payload of a transcript kind, that every Run has
/// ended, that every schedule and queued turn belongs to the Conversation,
/// and that every Artifact reference has its payload, exactly.
pub(crate) fn decode(bytes: &[u8]) -> Result<DecodedBundle, CoreError> {
	let mut parts = format::decode_in(&format::TRANSFER, bytes)
		.map_err(|_| invalid_bundle())?;
	let document = parts.remove("transfer.json").ok_or_else(invalid_bundle)?;
	let bundle: TransferBundle =
		serde_json::from_slice(&document).map_err(|_| invalid_bundle())?;
	if bundle.format != FORMAT {
		return Err(CoreError::incompatible(
			"transfer.format_incompatible",
			"the bundle was prepared by a core this one cannot read",
		));
	}
	validate(&bundle)?;
	let payloads: BTreeMap<String, Vec<u8>> = parts
		.into_iter()
		.map(|(name, bytes)| {
			(name.trim_start_matches("artifacts/").to_owned(), bytes)
		})
		.collect();
	let mut referenced: Vec<(&str, u64)> = bundle
		.artifacts
		.iter()
		.map(|artifact| (artifact.sha256.as_str(), artifact.size))
		.collect();
	referenced.sort_unstable();
	referenced.dedup();
	let supplied: Vec<(&str, u64)> = payloads
		.iter()
		.map(|(sha256, bytes)| (sha256.as_str(), bytes.len() as u64))
		.collect();
	if referenced != supplied {
		return Err(invalid_bundle());
	}
	Ok(DecodedBundle { bundle, payloads })
}

fn validate(bundle: &TransferBundle) -> Result<(), CoreError> {
	let conversation_id = bundle.conversation.conversation_id;
	if bundle.conversation.retired_epoch == 0
		|| bundle.source_plane_id == bundle.target_plane_id
		|| bundle.runs.len() > RUN_LIMIT
		|| bundle.events.len() > TRANSCRIPT_EVENT_LIMIT
		|| bundle.schedules.len() > SCHEDULE_LIMIT
		|| bundle.settings.len() > SETTING_LIMIT
		|| bundle.artifacts.len() > ARTIFACT_LIMIT
		|| !valid_name(&bundle.conversation.name)
	{
		return Err(invalid_bundle());
	}
	let mut runs = std::collections::BTreeSet::new();
	for run in &bundle.runs {
		if !run.lifecycle.is_terminal()
			|| !valid_name(&run.name)
			|| !runs.insert(run.run_id)
		{
			return Err(invalid_bundle());
		}
	}
	let mut events = std::collections::BTreeSet::new();
	for event in &bundle.events {
		if !TRANSCRIPT_KINDS.contains(&event.kind.as_str())
			|| event.payload_version != 1
			|| event.payload.len() > PAYLOAD_LIMIT
			|| serde_json::from_str::<serde_json::Value>(&event.payload)
				.is_err()
			|| event.run_id.is_some_and(|run_id| !runs.contains(&run_id))
			|| !events.insert(event.event_id)
		{
			return Err(invalid_bundle());
		}
	}
	if let Some(queue) = &bundle.turn_queue
		&& (queue.len() > STATE_LIMIT
			|| serde_json::from_str::<crate::turn::queue::Queue>(queue)
				.is_err())
	{
		return Err(invalid_bundle());
	}
	let mut schedules = std::collections::BTreeSet::new();
	for schedule in &bundle.schedules {
		let task: crate::ScheduledTask = (schedule.len() <= STATE_LIMIT)
			.then(|| serde_json::from_str(schedule).ok())
			.flatten()
			.ok_or_else(invalid_bundle)?;
		if task.conversation_id.0 != conversation_id
			|| !schedules.insert(task.schedule_id)
		{
			return Err(invalid_bundle());
		}
	}
	for setting in &bundle.settings {
		if setting.key.is_empty()
			|| setting.key.len() > 128
			|| setting.value.len() > STATE_LIMIT
		{
			return Err(invalid_bundle());
		}
	}
	for artifact in &bundle.artifacts {
		if !runs.contains(&artifact.run_id)
			|| format::artifact_name(&format!("artifacts/{}", artifact.sha256))
				.is_none()
		{
			return Err(invalid_bundle());
		}
	}
	Ok(())
}

fn valid_name(name: &Name) -> bool {
	!name.value.is_empty()
		&& name.value.len() <= MAX_NAME_BYTES
		&& !name.value.chars().any(char::is_control)
}

pub(crate) fn invalid_bundle() -> CoreError {
	CoreError::invalid_input(
		"transfer.invalid_bundle",
		"the transfer bundle is malformed or incomplete",
	)
}
