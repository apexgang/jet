//! The target's prepare phase: reading the bundle, publishing its
//! payloads, and recording the Conversation as a Prepared transfer that
//! cannot run work or fire schedules (ADR-0070).

use super::{PlaneTransfer, PlaneTransferId, bundle::DecodedBundle};
use crate::{
	Actor, ArtifactDescriptor, CommandOutcome, ConversationId, Core, CoreError,
	EventKind, PlaneId, ProjectId,
	audit::{self, AuditDecision, AuditSubject, Decision},
	checkpoint::ArtifactAvailability,
	event::EventSubject,
	plane_transfer::bundle,
};
use jet_store::{
	ActorRecord, AuthorityRecord, AuthorityState, EventClass, NewConversation,
	NewEvent, NewPlaneTransfer, NewRun, PlaneTransferRole, SettingRecord,
	SettingScopeRecord, WorkingTreeRecord, WriteTransaction,
};

/// A bundle read, checked, and mapped onto this Plane, with its payloads
/// published, waiting for the transaction that records it.
pub(crate) struct PreparedImport {
	decoded: DecodedBundle,
	bundle_sha256: String,
	working_tree: WorkingTreeRecord,
}

impl Core {
	/// Reads `bytes` and publishes its payloads before the transaction
	/// opens: the checks describe the bundle and the machine rather than
	/// the Command, and payload publication takes the filesystem.
	pub(crate) async fn prepare_plane_transfer_import(
		&self,
		bytes: &[u8],
		project_id: Option<ProjectId>,
	) -> Result<PreparedImport, CoreError> {
		let bundle_sha256 = crate::store_recovery::bundle::format::hash(bytes);
		let decoded = bundle::decode(bytes)?;
		let plane_id = self
			.store
			.read(async |tx| Ok::<_, CoreError>(tx.plane().await?.plane_id))
			.await?;
		if decoded.bundle.target_plane_id != plane_id {
			return Err(CoreError::invalid_input(
				"transfer.wrong_plane",
				"the bundle was prepared for another target Plane",
			));
		}
		let working_tree =
			match (decoded.bundle.conversation.in_project, project_id) {
				(_, Some(project_id)) => {
					self.store
						.read(async |tx| tx.project(project_id.0).await)
						.await?
						.ok_or_else(|| {
							CoreError::not_found(
								"project.not_found",
								"the Project is not registered on this Plane",
							)
						})?;
					WorkingTreeRecord::LocalCheckout {
						project_id: project_id.0,
					}
				}
				(true, None) => {
					return Err(CoreError::invalid_input(
						"transfer.project_required",
						"the Conversation worked in a Project; name the Project \
					 on this Plane it continues in",
					));
				}
				(false, None) => WorkingTreeRecord::NoProject,
			};
		let payload_bytes: u64 = decoded
			.payloads
			.values()
			.map(|bytes| bytes.len() as u64)
			.sum();
		self.check_disk(payload_bytes + 1024 * 1024).await?;
		self.publish_payloads(&decoded).await?;
		Ok(PreparedImport {
			decoded,
			bundle_sha256,
			working_tree,
		})
	}

	/// Publishes every payload the bundle carries under its hash, through
	/// the same verification and budget as an uploaded Artifact. A payload
	/// already published is left as it is.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "publication fence protects the payloads from collection while they are read or written"
	)]
	async fn publish_payloads(
		&self,
		decoded: &DecodedBundle,
	) -> Result<(), CoreError> {
		if decoded.payloads.is_empty() {
			return Ok(());
		}
		let _publication = self.artifact_publication.lock().await;
		let _file_lock =
			crate::artifact::files::publication_lock(self.run_home()).await?;
		let limits = self.artifact_policy().await?;
		for artifact in &decoded.bundle.artifacts {
			let Some(bytes) = decoded.payloads.get(&artifact.sha256) else {
				continue;
			};
			if artifact.size > limits.artifact_bytes {
				return Err(CoreError::invalid_input(
					"artifact.size_exceeded",
					"a transferred Artifact exceeds the ingestion limit",
				));
			}
			let home = self.run_home();
			let bytes = bytes.clone();
			let descriptor = ArtifactDescriptor {
				sha256: artifact.sha256.clone(),
				size: artifact.size,
			};
			let run_id = crate::RunId(artifact.run_id);
			let run_limit = limits.run_bytes;
			let free_reserve_bytes = limits.free_reserve_bytes;
			crate::filesystem::blocking(move || {
				use std::io::Write as _;
				let payloads = crate::artifact::files::directory(&home)?;
				if crate::artifact::files::open(&payloads, &descriptor.sha256)
					.and_then(|mut file| {
						crate::artifact::files::verify(&mut file, &descriptor)
					})
					.is_ok()
				{
					return Ok(());
				}
				let (pending, mut file) = crate::artifact::files::stage(&home)?;
				crate::artifact::files::reserve(
					&file,
					descriptor.size,
					free_reserve_bytes,
				)?;
				file.write_all(&bytes)
					.and_then(|()| file.sync_all())
					.map_err(crate::artifact::files::io_error)?;
				crate::artifact::files::verify(&mut file, &descriptor)?;
				match crate::checkpoint::change_artifact_budget::publish_with_limit(
					pending,
					run_id,
					&descriptor.sha256,
					descriptor.size,
					run_limit,
					crate::checkpoint::change_artifact::directory(&home)
						.map_err(crate::artifact::files::io_error)?,
				)? {
					ArtifactAvailability::Stored => Ok(()),
					ArtifactAvailability::DiskPressure => {
						Err(crate::disk_pressure::pressure())
					}
					ArtifactAvailability::RunBudgetExceeded
					| ArtifactAvailability::ArtifactSizeExceeded => {
						Err(CoreError::invalid_input(
							"artifact.run_budget_exceeded",
							"a transferred Run's Artifact budget is exhausted",
						))
					}
				}
			})
			.await??;
		}
		Ok(())
	}
}

/// Records the bundle as a Prepared transfer in one transaction: the
/// Conversation under its own identity in the epoch after the retired
/// one, its Runs, transcript, queued turns, schedules, Settings, and
/// Artifact references, and the transfer itself.
pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &Actor,
	prepared: PreparedImport,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	let PreparedImport {
		decoded: DecodedBundle { bundle, .. },
		bundle_sha256,
		working_tree,
	} = prepared;
	let conversation_id = ConversationId(bundle.conversation.conversation_id);
	let transfer_id = PlaneTransferId(bundle.transfer_id);
	if let Some(existing) = tx.plane_transfer(transfer_id.0).await? {
		super::require_bundle(&existing, &bundle_sha256)?;
		return Ok(CommandOutcome::PlaneTransferImported(PlaneTransfer::from(
			existing,
		)));
	}
	let epoch = bundle
		.conversation
		.retired_epoch
		.checked_add(1)
		.ok_or_else(bundle::invalid_bundle)?;
	if let Some(existing) = tx.conversation(conversation_id.0).await? {
		// The Conversation lived here before and left; its tombstone gives
		// way to the copy coming back in a later epoch.
		if existing.authority.state != AuthorityState::Relinquished
			|| existing.authority.epoch >= epoch
		{
			return Err(CoreError::conflict(
				"transfer.conversation_exists",
				"this Plane already holds the Conversation",
			));
		}
		tx.discard_transfer_tombstone(conversation_id.0).await?;
	}
	tx.insert_conversation(NewConversation {
		conversation_id: conversation_id.0,
		retention: bundle.conversation.retention,
		working_tree,
		origin: jet_store::ConversationOriginRecord::New,
		created_at_unix_ms: bundle.conversation.created_at_unix_ms,
	})
	.await?;
	tx.update_conversation_name(
		conversation_id.0,
		&bundle.conversation.name.into(),
	)
	.await?;
	tx.set_conversation_authority(
		conversation_id.0,
		AuthorityRecord {
			state: AuthorityState::Prepared,
			epoch,
		},
	)
	.await?;
	for run in bundle.runs {
		tx.insert_run(NewRun {
			run_id: run.run_id,
			conversation_id: conversation_id.0,
			created_at_unix_ms: run.created_at_unix_ms,
		})
		.await?;
		tx.update_run_lifecycle(
			run.run_id,
			run.lifecycle,
			run.ended_at_unix_ms.unwrap_or(run.created_at_unix_ms),
		)
		.await?;
		tx.update_run_name(run.run_id, &run.name.into()).await?;
	}
	for event in bundle.events {
		tx.append_event(NewEvent {
			event_id: event.event_id,
			actor: ActorRecord::InteractiveClient {
				client_id: event.client_id,
			},
			recorded_at_unix_ms: event.recorded_at_unix_ms,
			conversation_id: Some(conversation_id.0),
			run_id: event.run_id,
			kind: event.kind,
			payload_version: event.payload_version,
			payload: event.payload,
			class: EventClass::Semantic,
		})
		.await?;
	}
	for artifact in bundle.artifacts {
		tx.reference_artifact(
			artifact.run_id,
			&artifact.sha256,
			i64::try_from(artifact.size).unwrap_or(i64::MAX),
		)
		.await?;
	}
	if let Some(state) = bundle.turn_queue {
		let queue: crate::turn::queue::Queue = serde_json::from_str(&state)
			.map_err(|_| bundle::invalid_bundle())?;
		crate::turn::queue::save(tx, conversation_id, &queue).await?;
	}
	for schedule in bundle.schedules {
		let task: crate::ScheduledTask = serde_json::from_str(&schedule)
			.map_err(|_| bundle::invalid_bundle())?;
		crate::schedule::save(tx, &task).await?;
	}
	for setting in bundle.settings {
		tx.upsert_setting(&SettingRecord {
			key: setting.key,
			scope: SettingScopeRecord::Conversation {
				conversation_id: conversation_id.0,
			},
			value: setting.value,
			updated_at_unix_ms: setting.updated_at_unix_ms,
		})
		.await?;
	}
	let recorded = tx
		.insert_plane_transfer(NewPlaneTransfer {
			transfer_id: transfer_id.0,
			conversation_id: conversation_id.0,
			role: PlaneTransferRole::Target,
			retired_epoch: bundle.conversation.retired_epoch,
			peer_plane_id: bundle.source_plane_id,
			bundle_sha256,
			prepared_at_unix_ms: now,
		})
		.await?;
	tx.append_event(
		EventKind::ConversationTransferImported {
			conversation_id,
			transfer_id,
			source_plane_id: PlaneId(bundle.source_plane_id),
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
			AuditDecision::PlaneTransferImported,
			AuditSubject::Conversation(conversation_id),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::PlaneTransferImported(PlaneTransfer::from(
		recorded,
	)))
}
