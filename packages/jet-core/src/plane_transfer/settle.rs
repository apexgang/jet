//! The phases that settle a transfer: the source relinquishing, the target
//! committing, and the source aborting a transfer it prepared (ADR-0070).

use super::{
	AuthorityFence, PlaneTransfer, PlaneTransferId, TOMBSTONE_MS, recorded,
	require_bundle,
};
use crate::{
	Actor, CommandOutcome, ConversationId, CoreError, EventKind, PlaneId,
	audit::{self, AuditDecision, AuditSubject, Decision},
	event::EventSubject,
	retention::protection::{has_live_work, protections},
	system_time,
};
use jet_store::{
	AuthorityRecord, AuthorityState, PlaneTransferPhase, PlaneTransferRecord,
	PlaneTransferRole, WriteTransaction,
};

/// The source retires its authority: the fence reaches its ledger before
/// this transaction commits, the Conversation becomes the Transfer
/// tombstone, and the fence is returned for the target to validate. A
/// transfer already relinquished answers with the same fence.
pub(crate) async fn relinquish(
	tx: &mut WriteTransaction,
	actor: &Actor,
	transfer_id: PlaneTransferId,
	bundle_sha256: &str,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let transfer = recorded(tx, transfer_id, PlaneTransferRole::Source).await?;
	require_bundle(&transfer, bundle_sha256)?;
	let source_plane_id = PlaneId(tx.plane().await?.plane_id);
	match transfer.phase {
		PlaneTransferPhase::Relinquished => {
			return Ok(CommandOutcome::PlaneTransferRelinquished(fence(
				&transfer,
				source_plane_id,
			)));
		}
		PlaneTransferPhase::Prepared => {}
		PlaneTransferPhase::Committed => {
			return Err(CoreError::internal(
				"transfer.phase_invalid",
				"a source transfer cannot be committed",
			));
		}
	}
	let conversation_id = ConversationId(transfer.conversation_id);
	// The freeze refuses new work, but work admitted before the prepare
	// and still live would be lost with the authority.
	if has_live_work(
		&protections(tx, conversation_id, Default::default()).await?,
	) {
		return Err(CoreError::conflict(
			"transfer.live_work",
			"the Conversation has a live Run or queued turns; stop and \
			 withdraw them before relinquishing",
		));
	}
	tx.relinquish_plane_transfer(
		&transfer,
		now,
		now.saturating_add(TOMBSTONE_MS),
	)
	.await?;
	let transfer = PlaneTransferRecord {
		phase: PlaneTransferPhase::Relinquished,
		settled_at_unix_ms: Some(now),
		..transfer
	};
	let fence = fence(&transfer, source_plane_id);
	tx.append_event(
		EventKind::ConversationTransferRelinquished {
			conversation_id,
			fence: fence.clone(),
			tombstone_expires_at_unix_ms: now.saturating_add(TOMBSTONE_MS),
		}
		.to_record(
			actor,
			EventSubject::Conversation(conversation_id),
			now,
		)?,
	)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PlaneTransferRelinquished,
			AuditSubject::Conversation(conversation_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::PlaneTransferRelinquished(fence))
}

fn fence(transfer: &PlaneTransferRecord, source: PlaneId) -> AuthorityFence {
	AuthorityFence {
		conversation_id: ConversationId(transfer.conversation_id),
		retired_epoch: transfer.retired_epoch,
		transfer_id: PlaneTransferId(transfer.transfer_id),
		source_plane_id: source,
		target_plane_id: PlaneId(transfer.peer_plane_id),
		fenced_at: system_time(
			transfer
				.settled_at_unix_ms
				.unwrap_or(transfer.prepared_at_unix_ms),
		),
	}
}

/// The target takes authority, once the fence the source left names
/// exactly this transfer, this Conversation, the epoch being retired, this
/// Plane, and the source it was prepared from. A transfer already
/// committed answers again.
pub(crate) async fn commit(
	tx: &mut WriteTransaction,
	actor: &Actor,
	transfer_id: PlaneTransferId,
	bundle_sha256: &str,
	fence: &AuthorityFence,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let transfer = recorded(tx, transfer_id, PlaneTransferRole::Target).await?;
	require_bundle(&transfer, bundle_sha256)?;
	match transfer.phase {
		PlaneTransferPhase::Committed => {
			return Ok(CommandOutcome::PlaneTransferCommitted(
				PlaneTransfer::from(transfer),
			));
		}
		PlaneTransferPhase::Prepared => {}
		PlaneTransferPhase::Relinquished => {
			return Err(CoreError::internal(
				"transfer.phase_invalid",
				"a target transfer cannot be relinquished",
			));
		}
	}
	let plane_id = tx.plane().await?.plane_id;
	if fence.transfer_id != transfer_id
		|| fence.conversation_id.0 != transfer.conversation_id
		|| fence.retired_epoch != transfer.retired_epoch
		|| fence.source_plane_id.0 != transfer.peer_plane_id
		|| fence.target_plane_id.0 != plane_id
	{
		return Err(CoreError::conflict(
			"transfer.fence_invalid",
			"the Authority fence does not name this transfer, this \
			 Conversation, the retired epoch, this Plane, and its source",
		));
	}
	let conversation_id = ConversationId(transfer.conversation_id);
	let conversation = tx
		.conversation(conversation_id.0)
		.await?
		.ok_or_else(super::conversation_not_found)?;
	let epoch = transfer
		.retired_epoch
		.checked_add(1)
		.ok_or_else(super::bundle::invalid_bundle)?;
	if conversation.authority
		!= (AuthorityRecord {
			state: AuthorityState::Prepared,
			epoch,
		}) {
		return Err(super::not_authoritative());
	}
	tx.set_conversation_authority(
		conversation_id.0,
		AuthorityRecord {
			state: AuthorityState::Home,
			epoch,
		},
	)
	.await?;
	tx.settle_plane_transfer(transfer_id.0, PlaneTransferPhase::Committed, now)
		.await?;
	let transfer = PlaneTransferRecord {
		phase: PlaneTransferPhase::Committed,
		settled_at_unix_ms: Some(now),
		..transfer
	};
	tx.append_event(
		EventKind::ConversationTransferCommitted {
			conversation_id,
			transfer_id,
			source_plane_id: fence.source_plane_id,
			epoch,
		}
		.to_record(
			actor,
			EventSubject::Conversation(conversation_id),
			now,
		)?,
	)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PlaneTransferCommitted,
			AuditSubject::Conversation(conversation_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::PlaneTransferCommitted(PlaneTransfer::from(
		transfer,
	)))
}

/// The source gives up a transfer it prepared and has not relinquished:
/// the record goes, and the Conversation takes new work again. A target's
/// Prepared copy is not aborted here; it is forgotten through Jet Trash
/// like any Conversation.
pub(crate) async fn abort(
	tx: &mut WriteTransaction,
	actor: &Actor,
	transfer_id: PlaneTransferId,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let transfer = recorded(tx, transfer_id, PlaneTransferRole::Source).await?;
	match transfer.phase {
		PlaneTransferPhase::Prepared => {}
		PlaneTransferPhase::Relinquished | PlaneTransferPhase::Committed => {
			return Err(CoreError::conflict(
				"transfer.relinquished",
				"the source has relinquished its authority; the transfer \
				 can no longer be aborted",
			));
		}
	}
	tx.delete_plane_transfer(transfer_id.0).await?;
	let conversation_id = ConversationId(transfer.conversation_id);
	tx.append_event(
		EventKind::ConversationTransferAborted {
			conversation_id,
			transfer_id,
		}
		.to_record(
			actor,
			EventSubject::Conversation(conversation_id),
			now,
		)?,
	)
	.await?;
	audit::record(
		tx,
		actor,
		Decision::succeeded(
			AuditDecision::PlaneTransferAborted,
			AuditSubject::Conversation(conversation_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::PlaneTransferAborted { transfer_id })
}
