//! Plane transfer: moving one Conversation's Home Plane in phases, without
//! two simultaneously authoritative copies (ADR-0062, ADR-0070).
//!
//! The source prepares a bundle of the Conversation's durable state and
//! records the transfer, which freezes the Conversation there: no new
//! work is admitted and its schedules wait. The target imports the bundle
//! transactionally as a Prepared transfer, a copy that cannot run work or
//! fire schedules. The source then relinquishes, raising a permanent,
//! content-free Authority fence before its commit and leaving its content
//! as a thirty-day Transfer tombstone in Jet Trash. Only with that fence
//! in hand does the target commit, and only then is its copy the Home
//! Plane, in the epoch after the one the source retired. Every step is
//! retryable by the same identity and bundle hash; a prepared transfer
//! the source abandons is aborted explicitly.
//!
//! The controlling GUI carries the bundle and the fence between its two
//! authenticated Plane connections; the Planes never talk to each other.

mod bundle;
pub(crate) mod import;
mod prepare;
pub(crate) mod settle;

pub(crate) use import::PreparedImport;

use crate::{ConversationId, CoreError, PlaneId, system_time};
use jet_store::{
	AuthorityState, ConversationRecord, PlaneTransferPhase,
	PlaneTransferRecord, PlaneTransferRole, ReadTransaction,
};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

/// Durable identity of one Plane transfer, shared by both Planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaneTransferId(pub Uuid);

/// Which side of a transfer a Plane is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferRole {
	/// The Plane that prepared the bundle and relinquishes.
	Source,
	/// The Plane that imported the bundle and commits.
	Target,
}

/// How far a transfer has come on one Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
	/// The source has prepared the bundle, or the target has imported it.
	Prepared,
	/// The source has fenced its authority.
	Relinquished,
	/// The target has validated the fence and holds the new authority.
	Committed,
}

/// One Plane transfer as one Plane recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaneTransfer {
	/// The transfer's identity.
	pub transfer_id: PlaneTransferId,
	/// The Conversation whose authority moves.
	pub conversation_id: ConversationId,
	/// Which side this Plane is.
	pub role: TransferRole,
	/// How far it has come here.
	pub phase: TransferPhase,
	/// The source's epoch, which the transfer retires; the target's copy
	/// is in the epoch after it.
	pub retired_epoch: u64,
	/// The other Plane.
	pub peer_plane_id: PlaneId,
	/// SHA-256 of the bundle, in lowercase hex, which every later step
	/// presents.
	pub bundle_sha256: String,
	/// When this Plane recorded the transfer.
	pub prepared_at: SystemTime,
	/// When it was relinquished or committed here.
	pub settled_at: Option<SystemTime>,
}

impl From<PlaneTransferRecord> for PlaneTransfer {
	fn from(record: PlaneTransferRecord) -> Self {
		Self {
			transfer_id: PlaneTransferId(record.transfer_id),
			conversation_id: ConversationId(record.conversation_id),
			role: match record.role {
				PlaneTransferRole::Source => TransferRole::Source,
				PlaneTransferRole::Target => TransferRole::Target,
			},
			phase: match record.phase {
				PlaneTransferPhase::Prepared => TransferPhase::Prepared,
				PlaneTransferPhase::Relinquished => TransferPhase::Relinquished,
				PlaneTransferPhase::Committed => TransferPhase::Committed,
			},
			retired_epoch: record.retired_epoch,
			peer_plane_id: PlaneId(record.peer_plane_id),
			bundle_sha256: record.bundle_sha256,
			prepared_at: system_time(record.prepared_at_unix_ms),
			settled_at: record.settled_at_unix_ms.map(system_time),
		}
	}
}

/// A prepared transfer together with the bundle the target imports. The
/// bundle is returned directly and never stored in a receipt; a retry
/// prepares it again from the frozen Conversation and checks its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedPlaneTransfer {
	/// The transfer as the source recorded it.
	pub transfer: PlaneTransfer,
	/// The bundle, bounded to 256 MiB.
	pub bundle: Vec<u8>,
}

/// The content-free record a source leaves when it relinquishes: which
/// Conversation authority it retired, through which transfer, and to
/// which Plane. The target validates it against its Prepared transfer
/// before committing (ADR-0070).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityFence {
	/// The Conversation whose authority was retired.
	pub conversation_id: ConversationId,
	/// The epoch the source held and retired.
	pub retired_epoch: u64,
	/// The transfer that retired it.
	pub transfer_id: PlaneTransferId,
	/// The Plane that retired its authority.
	pub source_plane_id: PlaneId,
	/// The Plane that holds the next epoch.
	pub target_plane_id: PlaneId,
	/// When the source relinquished.
	pub fenced_at: SystemTime,
}

/// How long the source keeps a relinquished Conversation's content as a
/// Transfer tombstone before Jet Trash deletes it (ADR-0070).
pub(crate) const TOMBSTONE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Whether `conversation` may take on new work here: it must be this
/// Plane's own, in its Home authority, and not frozen by a transfer this
/// Plane has prepared as its source. Run admission, turn admission,
/// dispatch, and schedule firing all ask this first.
///
/// # Errors
///
/// Returns `conversation.not_authoritative` for a Prepared or
/// relinquished copy and `conversation.transfer_prepared` for a frozen
/// source, and a store category [`CoreError`] when the rows cannot be
/// read.
pub(crate) async fn require_authoritative(
	tx: &mut ReadTransaction,
	conversation: &ConversationRecord,
) -> Result<(), CoreError> {
	match conversation.authority.state {
		AuthorityState::Home => {}
		AuthorityState::Prepared | AuthorityState::Relinquished => {
			return Err(not_authoritative());
		}
	}
	if let Some(transfer) =
		tx.open_plane_transfer(conversation.conversation_id).await?
		&& transfer.role == PlaneTransferRole::Source
	{
		return Err(CoreError::conflict(
			"conversation.transfer_prepared",
			"the Conversation is prepared for a Plane transfer; commit it \
			 on the target or abort it before admitting new work",
		));
	}
	Ok(())
}

/// The same check for a Conversation known only by identity.
pub(crate) async fn require_authoritative_id(
	tx: &mut ReadTransaction,
	conversation_id: ConversationId,
) -> Result<(), CoreError> {
	let conversation = tx
		.conversation(conversation_id.0)
		.await?
		.ok_or_else(conversation_not_found)?;
	require_authoritative(tx, &conversation).await
}

/// Turns the gate's own refusals into a quiet pause for work that fires
/// on its own, a schedule or a dispatch, and lets any other error, a store
/// failure or a Conversation that is gone, through.
pub(crate) fn paused_by_transfer(error: CoreError) -> Result<(), CoreError> {
	if matches!(
		error.code.as_str(),
		"conversation.not_authoritative" | "conversation.transfer_prepared"
	) {
		Ok(())
	} else {
		Err(error)
	}
}

pub(crate) fn not_authoritative() -> CoreError {
	CoreError::conflict(
		"conversation.not_authoritative",
		"this Plane is not the Conversation's Home Plane",
	)
}

pub(crate) fn conversation_not_found() -> CoreError {
	CoreError::not_found(
		"conversation.not_found",
		"the Conversation does not exist",
	)
}

pub(crate) fn transfer_not_found() -> CoreError {
	CoreError::not_found(
		"transfer.not_found",
		"this Plane recorded no such Plane transfer",
	)
}

/// The transfer called `transfer_id` as this Plane recorded it, in the
/// role the caller expects.
pub(crate) async fn recorded(
	tx: &mut ReadTransaction,
	transfer_id: PlaneTransferId,
	role: PlaneTransferRole,
) -> Result<PlaneTransferRecord, CoreError> {
	let transfer = tx
		.plane_transfer(transfer_id.0)
		.await?
		.ok_or_else(transfer_not_found)?;
	if transfer.role != role {
		return Err(CoreError::conflict(
			"transfer.wrong_role",
			match role {
				PlaneTransferRole::Source => {
					"this Plane is the transfer's target, not its source"
				}
				PlaneTransferRole::Target => {
					"this Plane is the transfer's source, not its target"
				}
			},
		));
	}
	Ok(transfer)
}

/// Refuses a step presented with a hash other than the bundle's, so a
/// retry can only continue the transfer it was prepared with.
pub(crate) fn require_bundle(
	transfer: &PlaneTransferRecord,
	bundle_sha256: &str,
) -> Result<(), CoreError> {
	if transfer.bundle_sha256 != bundle_sha256 {
		return Err(CoreError::conflict(
			"transfer.bundle_mismatch",
			"the bundle hash is not the one this transfer was prepared with",
		));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Command, CommandOutcome, Core, EventKind, TurnSource,
		test_support::{actor, events, request, start_core},
		workspace::WorkingTreeRequest,
	};
	use jet_store::{
		ActorRecord, AuthorityRecord, EventClass, NewEvent, NewRun,
		RetentionPolicy, RunLifecycle, TrashReasonRecord,
	};
	use pretty_assertions::assert_eq;
	use std::time::SystemTime;

	/// Two Planes, each its own store under its own home.
	async fn planes(dir: &tempfile::TempDir) -> (Core, Core, PlaneId, PlaneId) {
		std::fs::create_dir_all(dir.path().join("source")).unwrap();
		std::fs::create_dir_all(dir.path().join("target")).unwrap();
		let source = start_core(&dir.path().join("source/plane.sqlite3")).await;
		let target = start_core(&dir.path().join("target/plane.sqlite3")).await;
		let source_id = plane_id(&source).await;
		let target_id = plane_id(&target).await;
		(source, target, source_id, target_id)
	}

	async fn plane_id(core: &Core) -> PlaneId {
		PlaneId(
			core.store
				.read(async |tx| Ok::<_, CoreError>(tx.plane().await?.plane_id))
				.await
				.unwrap(),
		)
	}

	async fn converse(core: &Core) -> ConversationId {
		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::NoProject,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("conversation");
		};
		conversation.conversation_id
	}

	/// A finished Run with one transcript Event, the way history looks
	/// once a Harness has run.
	async fn history(core: &Core, conversation_id: ConversationId) -> Uuid {
		let run_id = Uuid::now_v7();
		core.store
			.write(async |tx| {
				tx.insert_run(NewRun {
					run_id,
					conversation_id: conversation_id.0,
					created_at_unix_ms: 1_000,
				})
				.await?;
				tx.update_run_lifecycle(run_id, RunLifecycle::Completed, 2_000)
					.await?;
				tx.append_event(NewEvent {
					event_id: Uuid::now_v7(),
					actor: ActorRecord::InteractiveClient {
						client_id: Uuid::nil(),
					},
					recorded_at_unix_ms: 1_500,
					conversation_id: Some(conversation_id.0),
					run_id: Some(run_id),
					kind: "turn.input".into(),
					payload_version: 1,
					payload: r#"{"text":"hello","turn_id":"t1"}"#.into(),
					class: EventClass::Semantic,
				})
				.await?;
				Ok::<_, CoreError>(())
			})
			.await
			.unwrap();
		run_id
	}

	async fn submit(
		core: &Core,
		conversation_id: ConversationId,
	) -> Result<CommandOutcome, CoreError> {
		core.execute(
			&actor(),
			request(Command::SubmitTurn {
				conversation_id,
				source: TurnSource::User,
				prompt: "next".into(),
			}),
		)
		.await
	}

	async fn prepare(
		core: &Core,
		conversation_id: ConversationId,
		target: PlaneId,
	) -> Result<PreparedPlaneTransfer, CoreError> {
		match core
			.execute(
				&actor(),
				request(Command::PreparePlaneTransfer {
					conversation_id,
					target_plane_id: target,
				}),
			)
			.await?
		{
			CommandOutcome::PlaneTransferPrepared(prepared) => Ok(prepared),
			other => panic!("unexpected outcome {other:?}"),
		}
	}

	async fn import(
		core: &Core,
		bundle: Vec<u8>,
	) -> Result<PlaneTransfer, CoreError> {
		match core
			.execute(
				&actor(),
				request(Command::ImportPlaneTransfer {
					bundle,
					project_id: None,
				}),
			)
			.await?
		{
			CommandOutcome::PlaneTransferImported(transfer) => Ok(transfer),
			other => panic!("unexpected outcome {other:?}"),
		}
	}

	async fn relinquish(
		core: &Core,
		transfer: &PlaneTransfer,
	) -> Result<AuthorityFence, CoreError> {
		match core
			.execute(
				&actor(),
				request(Command::RelinquishPlaneTransfer {
					transfer_id: transfer.transfer_id,
					bundle_sha256: transfer.bundle_sha256.clone(),
				}),
			)
			.await?
		{
			CommandOutcome::PlaneTransferRelinquished(fence) => Ok(fence),
			other => panic!("unexpected outcome {other:?}"),
		}
	}

	async fn commit(
		core: &Core,
		transfer: &PlaneTransfer,
		fence: AuthorityFence,
	) -> Result<PlaneTransfer, CoreError> {
		match core
			.execute(
				&actor(),
				request(Command::CommitPlaneTransfer {
					transfer_id: transfer.transfer_id,
					bundle_sha256: transfer.bundle_sha256.clone(),
					fence,
				}),
			)
			.await?
		{
			CommandOutcome::PlaneTransferCommitted(transfer) => Ok(transfer),
			other => panic!("unexpected outcome {other:?}"),
		}
	}

	async fn authority(
		core: &Core,
		conversation_id: ConversationId,
	) -> AuthorityRecord {
		core.store
			.read(async |tx| {
				Ok::<_, CoreError>(
					tx.conversation(conversation_id.0)
						.await?
						.expect("the Conversation exists")
						.authority,
				)
			})
			.await
			.unwrap()
	}

	fn codes(events: &[EventKind]) -> Vec<String> {
		events
			.iter()
			.filter_map(|event| {
				serde_json::to_value(event)
					.ok()?
					.get("kind")?
					.as_str()
					.map(ToOwned::to_owned)
			})
			.filter(|kind| kind.starts_with("conversation.transfer"))
			.collect()
	}

	/// The whole transfer, phase by phase: the source freezes and bundles,
	/// the target imports a copy that admits no work, the source fences
	/// and keeps a tombstone, the target commits into the next epoch and
	/// takes work, and the source takes none (ADR-0070).
	#[tokio::test]
	async fn authority_moves_through_prepare_relinquish_and_commit() {
		let dir = tempfile::tempdir().unwrap();
		let (source, target, source_id, target_id) = planes(&dir).await;
		let conversation_id = converse(&source).await;
		let run_id = history(&source, conversation_id).await;

		let prepared =
			prepare(&source, conversation_id, target_id).await.unwrap();
		let frozen_source = submit(&source, conversation_id).await.unwrap_err();
		let again = prepare(&source, conversation_id, target_id).await.unwrap();
		let imported = import(&target, prepared.bundle.clone()).await.unwrap();
		let frozen_target = submit(&target, conversation_id).await.unwrap_err();
		let forged = commit(
			&target,
			&imported,
			AuthorityFence {
				conversation_id,
				retired_epoch: 1,
				transfer_id: imported.transfer_id,
				source_plane_id: source_id,
				target_plane_id: source_id,
				fenced_at: SystemTime::UNIX_EPOCH,
			},
		)
		.await
		.unwrap_err();
		let fence = relinquish(&source, &prepared.transfer).await.unwrap();
		let fence_again =
			relinquish(&source, &prepared.transfer).await.unwrap();
		let committed =
			commit(&target, &imported, fence.clone()).await.unwrap();
		let target_runs = target
			.store
			.read(async |tx| tx.runs(conversation_id.0).await)
			.await
			.unwrap();
		let target_transcript = target
			.store
			.read(async |tx| tx.transcript_events(conversation_id.0).await)
			.await
			.unwrap();
		let target_works = submit(&target, conversation_id).await.is_ok();
		let source_refuses =
			submit(&source, conversation_id).await.unwrap_err();
		let restore_refused = source
			.execute(
				&actor(),
				request(Command::RestoreConversation { conversation_id }),
			)
			.await
			.unwrap_err();
		let tombstone = source
			.store
			.read(async |tx| tx.trash_entry(conversation_id.0).await)
			.await
			.unwrap()
			.map(|entry| entry.reason);

		assert_eq!(
			(
				(
					prepared.transfer.role,
					prepared.transfer.phase,
					again.transfer.transfer_id,
					again.bundle == prepared.bundle,
					frozen_source.code,
				),
				(
					(imported.role, imported.phase, imported.retired_epoch),
					imported.peer_plane_id,
					imported.bundle_sha256 == prepared.transfer.bundle_sha256,
					frozen_target.code,
					forged.code,
				),
				(
					(
						fence.retired_epoch,
						fence.source_plane_id,
						fence.target_plane_id
					),
					fence_again,
					(committed.phase, committed.settled_at.is_some()),
					authority(&target, conversation_id).await,
					authority(&source, conversation_id).await,
				),
				(
					target_works,
					source_refuses.code,
					restore_refused.code,
					target_runs
						.iter()
						.map(|run| run.run_id)
						.collect::<Vec<_>>(),
					target_transcript.len(),
					tombstone,
					codes(&events(&source).await),
					codes(&events(&target).await),
				),
			),
			(
				(
					TransferRole::Source,
					TransferPhase::Prepared,
					prepared.transfer.transfer_id,
					true,
					"conversation.transfer_prepared".to_string(),
				),
				(
					(TransferRole::Target, TransferPhase::Prepared, 1),
					source_id,
					true,
					"conversation.not_authoritative".to_string(),
					"transfer.fence_invalid".to_string(),
				),
				(
					(1, source_id, target_id),
					fence.clone(),
					(TransferPhase::Committed, true),
					AuthorityRecord {
						state: jet_store::AuthorityState::Home,
						epoch: 2,
					},
					AuthorityRecord {
						state: jet_store::AuthorityState::Relinquished,
						epoch: 1,
					},
				),
				(
					true,
					"conversation.not_authoritative".to_string(),
					"retention.transferred".to_string(),
					vec![run_id],
					1,
					Some(TrashReasonRecord::Transferred),
					vec![
						"conversation.transfer_prepared".to_string(),
						"conversation.transfer_relinquished".to_string(),
					],
					vec![
						"conversation.transfer_imported".to_string(),
						"conversation.transfer_committed".to_string(),
					],
				),
			)
		);
	}

	/// Each step is retryable and refuses anything but the bundle it was
	/// prepared with: a second import answers the same transfer, a
	/// tampered bundle is refused, a wrong hash stops the settlement, and a
	/// prepared transfer is abandoned explicitly, after which the source
	/// takes work again.
	#[tokio::test]
	async fn steps_retry_by_bundle_hash_and_a_prepared_transfer_can_be_aborted()
	{
		let dir = tempfile::tempdir().unwrap();
		let (source, target, _, target_id) = planes(&dir).await;
		let conversation_id = converse(&source).await;
		let prepared =
			prepare(&source, conversation_id, target_id).await.unwrap();
		let mut tampered = prepared.bundle.clone();
		let last = tampered.len() - 1;
		tampered[last] ^= 0xff;
		let tampered = import(&target, tampered).await.unwrap_err();
		let imported = import(&target, prepared.bundle.clone()).await.unwrap();
		let imported_again =
			import(&target, prepared.bundle.clone()).await.unwrap();
		let wrong_hash = source
			.execute(
				&actor(),
				request(Command::RelinquishPlaneTransfer {
					transfer_id: prepared.transfer.transfer_id,
					bundle_sha256: "0".repeat(64),
				}),
			)
			.await
			.unwrap_err();
		let live_target = target
			.execute(
				&actor(),
				request(Command::AbortPlaneTransfer {
					transfer_id: imported.transfer_id,
				}),
			)
			.await
			.unwrap_err();
		let aborted = source
			.execute(
				&actor(),
				request(Command::AbortPlaneTransfer {
					transfer_id: prepared.transfer.transfer_id,
				}),
			)
			.await
			.unwrap();
		let source_works = submit(&source, conversation_id).await.is_ok();
		let gone = source
			.execute(
				&actor(),
				request(Command::RelinquishPlaneTransfer {
					transfer_id: prepared.transfer.transfer_id,
					bundle_sha256: prepared.transfer.bundle_sha256.clone(),
				}),
			)
			.await
			.unwrap_err();

		assert_eq!(
			(
				tampered.code,
				imported_again,
				wrong_hash.code,
				live_target.code,
				aborted,
				source_works,
				gone.code,
			),
			(
				"transfer.invalid_bundle".to_string(),
				imported,
				"transfer.bundle_mismatch".to_string(),
				"transfer.wrong_role".to_string(),
				CommandOutcome::PlaneTransferAborted {
					transfer_id: prepared.transfer.transfer_id,
				},
				true,
				"transfer.not_found".to_string(),
			)
		);
	}

	/// Live work refuses a prepare, and a Conversation cannot be moved to
	/// the Plane it is on.
	#[tokio::test]
	async fn live_work_and_the_same_plane_refuse_a_prepare() {
		let dir = tempfile::tempdir().unwrap();
		let (source, _, source_id, target_id) = planes(&dir).await;
		let conversation_id = converse(&source).await;
		submit(&source, conversation_id).await.unwrap();
		let live = prepare(&source, conversation_id, target_id)
			.await
			.unwrap_err();
		let same = prepare(&source, conversation_id, source_id)
			.await
			.unwrap_err();
		assert_eq!(
			(live.code, same.code),
			(
				"transfer.live_work".to_string(),
				"transfer.same_plane".to_string()
			)
		);
	}
}
