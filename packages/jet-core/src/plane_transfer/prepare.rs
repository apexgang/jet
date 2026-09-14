//! The source's prepare phase: freezing the Conversation and producing the
//! bundle the target imports (ADR-0070).
//!
//! Preparing runs beside the receipt pipeline, like a Recovery export,
//! because the bundle is returned directly and never stored in a receipt.
//! It is idempotent by state instead: while the transfer stays prepared,
//! every prepare of the same Conversation to the same target reads the
//! frozen Conversation again, produces the same bytes, and checks their
//! hash against the one recorded first.

use super::{PlaneTransfer, PreparedPlaneTransfer, bundle};
use crate::{
	Actor, CommandOutcome, ConversationId, Core, CoreError, EventKind, PlaneId,
	audit::{self, AuditDecision, AuditSubject, Decision},
	event::EventSubject,
	retention::protection::{has_live_work, protections},
	store_recovery::bundle::format,
};
use jet_store::{
	AuthorityState, NewPlaneTransfer, PlaneTransferPhase, PlaneTransferRecord,
	PlaneTransferRole, ReadTransaction,
};
use uuid::Uuid;

impl Core {
	/// Prepares a Plane transfer of `conversation_id` to `target_plane_id`
	/// and returns the bundle the target imports; see the module
	/// documentation.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "publication fence protects the payloads from collection while they are read or written"
	)]
	pub(crate) async fn prepare_plane_transfer(
		&self,
		actor: &Actor,
		conversation_id: ConversationId,
		target_plane_id: PlaneId,
	) -> Result<CommandOutcome, CoreError> {
		let _access = self
			.remote_access
			.acquire_many(crate::remote::AUTHORITY_READERS)
			.await
			.expect("authority gate never closes");
		actor.authorize(&self.remote_sessions)?;
		if let crate::RecoveryMode::ReadOnly(reason) = self.recovery_mode() {
			return Err(crate::store_recovery::read_only(reason));
		}
		self.security
			.read()
			.await
			.admit(crate::security::SecurityClass::Guarded)?;
		let now = self.now_unix_ms();
		// What the bundle carries is read in one snapshot, before the row
		// that freezes the Conversation exists; the write below notices a
		// Conversation that moved in between and asks for a retry.
		let (planned, bundle) = self
			.store
			.read(async |tx| {
				let planned =
					plan(tx, conversation_id, target_plane_id, now).await?;
				let source = tx.plane().await?.plane_id;
				let bundle =
					bundle::capture(tx, &planned.transfer, source).await?;
				Ok::<_, CoreError>((planned, bundle))
			})
			.await?;
		// Match Artifact publication's lock order so no payload the bundle
		// names is collected while it is read.
		let _publication = self.artifact_publication.lock().await;
		let _file_lock =
			crate::artifact::files::publication_lock(self.run_home()).await?;
		let home = self.run_home();
		let bytes = crate::filesystem::blocking(move || {
			let payloads = crate::artifact::files::directory(&home)?;
			bundle::encode(&bundle, &payloads)
		})
		.await??;
		drop(_file_lock);
		drop(_publication);
		let bundle_sha256 = format::hash(&bytes);
		let transfer = self
			.store
			.write(async |tx| {
				let current =
					plan(tx, conversation_id, target_plane_id, now).await?;
				if current.recorded != planned.recorded
					|| current.revision != planned.revision
					|| (planned.recorded
						&& current.transfer.transfer_id
							!= planned.transfer.transfer_id)
				{
					return Err(CoreError::conflict(
						"transfer.stale",
						"the Conversation changed while its bundle was \
						 prepared; prepare it again",
					));
				}
				if planned.recorded {
					super::require_bundle(&current.transfer, &bundle_sha256)
						.map_err(|_| {
							CoreError::conflict(
								"transfer.bundle_drift",
								"the frozen Conversation no longer produces \
								 the bundle this transfer was prepared with; \
								 abort the transfer and prepare it again",
							)
						})?;
					return Ok(current.transfer);
				}
				let recorded = tx
					.insert_plane_transfer(NewPlaneTransfer {
						transfer_id: planned.transfer.transfer_id,
						conversation_id: conversation_id.0,
						role: PlaneTransferRole::Source,
						retired_epoch: planned.transfer.retired_epoch,
						peer_plane_id: target_plane_id.0,
						bundle_sha256: bundle_sha256.clone(),
						prepared_at_unix_ms: now,
					})
					.await?;
				tx.append_event(
					EventKind::ConversationTransferPrepared {
						conversation_id,
						transfer_id: super::PlaneTransferId(
							recorded.transfer_id,
						),
						target_plane_id,
						retired_epoch: recorded.retired_epoch,
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
						AuditDecision::PlaneTransferPrepared,
						AuditSubject::Conversation(conversation_id),
					),
					now,
				)
				.await?;
				Ok::<_, CoreError>(recorded)
			})
			.await?;
		Ok(CommandOutcome::PlaneTransferPrepared(
			PreparedPlaneTransfer {
				transfer: PlaneTransfer::from(transfer),
				bundle: bytes,
			},
		))
	}
}

/// What a prepare of one Conversation continues or begins.
struct Planned {
	/// The transfer: the one already recorded to the same target, or the
	/// one to record, with its identity chosen and its hash still empty.
	transfer: PlaneTransferRecord,
	/// Whether the store already holds it.
	recorded: bool,
	/// The Conversation's Revision when it was read, so a write can tell
	/// whether the Conversation moved since.
	revision: u64,
}

/// The transfer a prepare of `conversation_id` to `target` continues or
/// begins. A transfer already prepared to the same target is continued;
/// anything else that makes the Conversation unfit to move is refused
/// here, by name.
async fn plan(
	tx: &mut ReadTransaction,
	conversation_id: ConversationId,
	target: PlaneId,
	now: i64,
) -> Result<Planned, CoreError> {
	let conversation = tx
		.conversation(conversation_id.0)
		.await?
		.ok_or_else(super::conversation_not_found)?;
	if tx.plane().await?.plane_id == target.0 {
		return Err(CoreError::invalid_input(
			"transfer.same_plane",
			"the target Plane is this Plane",
		));
	}
	match conversation.authority.state {
		AuthorityState::Home => {}
		AuthorityState::Prepared | AuthorityState::Relinquished => {
			return Err(super::not_authoritative());
		}
	}
	if let Some(open) = tx.open_plane_transfer(conversation_id.0).await? {
		if open.role == PlaneTransferRole::Source
			&& open.peer_plane_id == target.0
		{
			return Ok(Planned {
				transfer: open,
				recorded: true,
				revision: conversation.revision,
			});
		}
		return Err(CoreError::conflict(
			"transfer.in_progress",
			"the Conversation is already prepared for another Plane \
			 transfer; abort it first",
		));
	}
	if has_live_work(
		&protections(tx, conversation_id, Default::default()).await?,
	) {
		return Err(CoreError::conflict(
			"transfer.live_work",
			"the Conversation has a live Run or queued turns; stop and \
			 withdraw them before moving its Home Plane",
		));
	}
	if tx.trash_entry(conversation_id.0).await?.is_some() {
		return Err(CoreError::conflict(
			"transfer.trashed",
			"the Conversation is in Jet Trash; restore it before moving \
			 its Home Plane",
		));
	}
	Ok(Planned {
		transfer: PlaneTransferRecord {
			transfer_id: Uuid::now_v7(),
			conversation_id: conversation_id.0,
			role: PlaneTransferRole::Source,
			phase: PlaneTransferPhase::Prepared,
			retired_epoch: conversation.authority.epoch,
			peer_plane_id: target.0,
			bundle_sha256: String::new(),
			prepared_at_unix_ms: now,
			settled_at_unix_ms: None,
		},
		recorded: false,
		revision: conversation.revision,
	})
}
