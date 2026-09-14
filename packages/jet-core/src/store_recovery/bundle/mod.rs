//! Portable Recovery bundles (ADR-0074).

pub(crate) mod crypto;
pub(crate) mod format;
pub use crypto::{RECOVERY_PLAINTEXT_WARNING, RecoveryKey, RecoveryProtection};
mod import;
use crate::{Actor, CommandOutcome, Core, CoreError};
pub use import::{RecoveredBundle, RecoveredConversation};
use std::io::Read;

impl Core {
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "publication fence protects the snapshot's referenced files from collection"
	)]
	pub(crate) async fn export_recovery_bundle(
		&self,
		actor: &Actor,
		protection: RecoveryProtection,
	) -> Result<CommandOutcome, CoreError> {
		// Match Artifact publication's lock order before acquiring live authority.
		let _publication = self.artifact_publication.lock().await;
		let _file_lock =
			crate::artifact::files::publication_lock(self.run_home()).await?;
		let _access = self
			.remote_access
			.acquire_many(crate::remote::AUTHORITY_READERS)
			.await
			.expect("authority gate");
		actor.authorize(&self.remote_sessions)?;
		let plaintext = protection.is_unencrypted();
		if plaintext {
			self.security
				.read()
				.await
				.admit(crate::security::SecurityClass::Guarded)?;
		}
		let _crafts = self.craft_artifact_publication.lock().await;
		let directory =
			tempfile::tempdir().map_err(crate::artifact::files::io_error)?;
		let path = directory.path().join("state.sqlite3");
		self.store.portable_snapshot(&path).await?;
		let artifacts = jet_store::Store::portable_artifacts(&path).await?;
		let crafts = self
			.observe_capabilities()
			.await
			.crafts
			.into_iter()
			.map(|craft| format::CraftMetadata {
				craft: craft.craft.0,
				version: craft.version,
				harnesses: craft.harnesses.into_iter().map(|h| h.0).collect(),
			})
			.collect::<Vec<_>>();
		let home = self.run_home();
		let bytes = crate::filesystem::blocking(move || {
			let mut parts = format::Components::new();
			let mut remaining = format::MAX_BYTES;
			let state = read_bounded(
				std::fs::File::open(path)
					.map_err(crate::artifact::files::io_error)?,
				&mut remaining,
			)?;
			parts.insert("state.sqlite3".into(), state);
			parts.insert(
				"crafts.json".into(),
				serde_json::to_vec(&crafts).map_err(|_| crypto::invalid())?,
			);
			let payloads = crate::artifact::files::directory(&home)?;
			for (sha256, size) in artifacts {
				let mut file =
					crate::artifact::files::open(&payloads, &sha256)?;
				crate::artifact::files::verify(
					&mut file,
					&crate::ArtifactDescriptor {
						sha256: sha256.clone(),
						size,
					},
				)?;
				let bytes = read_bounded(file.by_ref(), &mut remaining)?;
				if bytes.len() as u64 != size || format::hash(&bytes) != sha256
				{
					return Err(crypto::invalid());
				}
				parts.insert(format!("artifacts/{sha256}"), bytes);
			}
			crypto::encrypt(&format::encode(parts)?, protection)
		})
		.await??;
		if plaintext {
			self.authorize_bundle_action(
				actor,
				crate::AuditDecision::UnencryptedRecoveryExportAuthorized,
			)
			.await?;
		}
		Ok(CommandOutcome::RecoveryBundle(bytes))
	}
}
impl Core {
	async fn authorize_bundle_action(
		&self,
		actor: &Actor,
		decision: crate::AuditDecision,
	) -> Result<(), CoreError> {
		self.store
			.write(async |tx| {
				crate::audit::record(
					tx,
					actor,
					crate::audit::Decision::succeeded(
						decision,
						crate::audit::AuditSubject::Plane,
					),
					self.now_unix_ms(),
				)
				.await
			})
			.await
	}
}

pub(crate) fn read_bounded(
	mut reader: impl std::io::Read,
	remaining: &mut usize,
) -> Result<Vec<u8>, CoreError> {
	let mut bytes = Vec::new();
	reader
		.by_ref()
		.take(*remaining as u64 + 1)
		.read_to_end(&mut bytes)
		.map_err(crate::artifact::files::io_error)?;
	*remaining = remaining
		.checked_sub(bytes.len())
		.ok_or_else(crypto::invalid)?;
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	use crate::{
		Command, CommandOutcome, RecoveryProtection,
		test_support::{actor, request, start_core},
	};
	use pretty_assertions::assert_eq;

	#[tokio::test]
	async fn export_encrypts_the_snapshot_by_default() {
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::passphrase(
						"test recovery password".into(),
					)
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("encrypted bundle")
		};
		assert!(bytes.starts_with(b"age-encryption.org/v1\n"));
		assert!(!bytes.windows(16).any(|part| part == b"SQLite format 3\0"));
	}
	#[tokio::test]
	async fn import_stages_fresh_recovered_copies_without_changing_live_conversations()
	 {
		use crate::{Query, RecoveryKey, RetentionPolicy, WorkingTreeRequest};
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
		let CommandOutcome::ConversationCreated(original) = core
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
			panic!("Conversation")
		};
		core.execute(
			&actor(),
			request(Command::CreateSchedule {
				conversation_id: original.conversation_id,
				time_zone: "UTC".into(),
				local_time: "12:00:00".into(),
				prompt: "retained schedule".into(),
			}),
		)
		.await
		.unwrap();
		let before = core.query(&actor(), Query::Conversations).await.unwrap();
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::passphrase(
						"recovery password".into(),
					)
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("bundle")
		};
		let CommandOutcome::RecoveredBundle(recovered) = core
			.execute(
				&actor(),
				request(Command::ImportRecoveryBundle {
					bytes,
					key: RecoveryKey::passphrase("recovery password".into())
						.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Recovered copies")
		};
		assert_eq!(recovered.conversations.len(), 1);
		assert_eq!(recovered.conversations[0].source, original.conversation_id);
		assert_ne!(
			recovered.conversations[0].recovered,
			original.conversation_id
		);
		assert_eq!(recovered.disabled_schedules, 1);
		assert_eq!(
			core.query(&actor(), Query::Conversations).await.unwrap(),
			before
		);
		assert!(recovered.directory.join("state.sqlite3").is_file());
		assert!(
			jet_store::Store::open(&recovered.directory.join("state.sqlite3"))
				.await
				.is_err()
		);
	}

	#[tokio::test]
	async fn portable_bundles_verify_and_retain_referenced_artifacts_and_craft_metadata()
	 {
		use crate::{
			ArtifactDescriptor, RecoveryKey, RetentionPolicy,
			WorkingTreeRequest,
		};
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
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
			panic!("Conversation")
		};
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: conversation.conversation_id,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		let expected = ArtifactDescriptor { sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(), size: 3 };
		let mut upload = core
			.begin_artifact_upload(&actor(), run.run_id, expected.clone())
			.await
			.unwrap();
		upload.write_chunk(b"abc").await.unwrap();
		core.publish_artifact(&actor(), upload).await.unwrap();
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::passphrase(
						"recovery password".into(),
					)
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("bundle")
		};
		let CommandOutcome::RecoveredBundle(recovered) = core
			.execute(
				&actor(),
				request(Command::ImportRecoveryBundle {
					bytes,
					key: RecoveryKey::passphrase("recovery password".into())
						.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Recovered copies")
		};
		assert_eq!(recovered.artifacts, vec![expected.clone()]);
		assert_eq!(
			std::fs::read(
				recovered.directory.join("artifacts").join(&expected.sha256)
			)
			.unwrap(),
			b"abc"
		);
		assert!(recovered.directory.join("crafts.json").is_file());
	}

	#[tokio::test]
	async fn x25519_export_round_trips_and_tampered_ciphertext_never_imports() {
		use crate::RecoveryKey;
		use age::secrecy::ExposeSecret;
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
		let identity = age::x25519::Identity::generate();
		let key = RecoveryKey::identity(
			identity.to_string().expose_secret().to_owned(),
		)
		.unwrap();
		assert!(
			RecoveryProtection::recipients(vec![
				"ssh-ed25519 unsupported".into()
			])
			.is_err()
		);
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::recipients(vec![
						identity.to_public().to_string(),
					])
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("bundle")
		};
		let mut tampered = bytes.clone();
		*tampered.last_mut().unwrap() ^= 1;
		assert!(
			core.execute(
				&actor(),
				request(Command::ImportRecoveryBundle {
					bytes: tampered,
					key: key.clone()
				})
			)
			.await
			.is_err()
		);
		assert!(!directory.path().join("recovery/imports").exists());
		assert!(matches!(
			core.execute(
				&actor(),
				request(Command::ImportRecoveryBundle { bytes, key })
			)
			.await
			.unwrap(),
			CommandOutcome::RecoveredBundle(_)
		));
	}

	#[tokio::test]
	async fn plaintext_export_requires_the_disclosed_warning_and_records_authorization()
	 {
		use crate::{
			AuditSequence, ProviderId, Query, QueryResult,
			RECOVERY_PLAINTEXT_WARNING, RecoveryKey,
		};
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
		let QueryResult::Status(status) =
			core.query(&actor(), Query::Status).await.unwrap()
		else {
			panic!("status")
		};
		crate::test_support::bind_native_account(
			&core,
			ProviderId("excluded-account-marker".into()),
		)
		.await;
		assert!(RecoveryProtection::unencrypted("yes".into()).is_err());
		let protection =
			RecoveryProtection::unencrypted(RECOVERY_PLAINTEXT_WARNING.into())
				.unwrap();
		let command = Command::ExportRecoveryBundle { protection };
		assert!(
			serde_json::to_string(&command)
				.unwrap()
				.contains(RECOVERY_PLAINTEXT_WARNING)
		);
		let CommandOutcome::RecoveryBundle(bytes) =
			core.execute(&actor(), request(command)).await.unwrap()
		else {
			panic!("bundle")
		};
		assert!(bytes.starts_with(b"JET-RECOVERY"));
		for forbidden in [
			"excluded-account-marker".to_owned(),
			status.plane_id.0.to_string(),
		] {
			assert!(
				!bytes
					.windows(forbidden.len())
					.any(|part| part == forbidden.as_bytes())
			);
		}
		let QueryResult::SecurityAudit(page) = core
			.query(
				&actor(),
				Query::SecurityAudit {
					after: AuditSequence(0),
				},
			)
			.await
			.unwrap()
		else {
			panic!("audit")
		};
		assert!(
			page.entries.iter().any(|entry| entry.decision
				== "recovery.unencrypted_export_authorized")
		);
		assert!(matches!(
			core.execute(
				&actor(),
				request(Command::ImportRecoveryBundle {
					bytes,
					key: RecoveryKey::unencrypted()
				})
			)
			.await
			.unwrap(),
			CommandOutcome::RecoveredBundle(_)
		));
	}

	#[tokio::test]
	async fn an_import_command_replays_its_durable_result_after_restart() {
		use crate::RecoveryKey;
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("plane.sqlite3");
		let core = start_core(&path).await;
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::unencrypted(
						crate::RECOVERY_PLAINTEXT_WARNING.into(),
					)
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("bundle")
		};
		let command_id = crate::CommandId(uuid::Uuid::new_v4());
		let envelope = crate::CommandEnvelope::new(
			command_id,
			Command::ImportRecoveryBundle {
				bytes: bytes.clone(),
				key: RecoveryKey::unencrypted(),
			},
			b"ephemeral request encoding",
		)
		.unwrap();
		let original = core.execute(&actor(), envelope.clone()).await.unwrap();
		assert_eq!(
			core.execute(&actor(), envelope.clone()).await.unwrap(),
			original
		);
		let changed_encoding = crate::CommandEnvelope::new(
			command_id,
			Command::ImportRecoveryBundle {
				bytes: bytes.clone(),
				key: RecoveryKey::unencrypted(),
			},
			b"different ephemeral encoding",
		)
		.unwrap();
		assert_eq!(
			core.execute(&actor(), changed_encoding).await.unwrap(),
			original
		);
		let changed_bundle = crate::CommandEnvelope::new(
			command_id,
			Command::ImportRecoveryBundle {
				bytes: vec![0],
				key: RecoveryKey::unencrypted(),
			},
			b"",
		)
		.unwrap();
		assert_eq!(
			core.execute(&actor(), changed_bundle)
				.await
				.unwrap_err()
				.code,
			"recovery.command_reused"
		);
		assert_eq!(
			serde_json::to_string(
				&RecoveryKey::passphrase("one password".into()).unwrap()
			)
			.unwrap(),
			serde_json::to_string(
				&RecoveryKey::passphrase("another password".into()).unwrap()
			)
			.unwrap()
		);
		core.close().await;
		let restarted = start_core(&path).await;
		assert_eq!(
			restarted.execute(&actor(), envelope).await.unwrap(),
			original
		);
	}

	#[tokio::test]
	async fn import_respects_disk_reserve_and_rejects_component_tampering() {
		use crate::{ArtifactLimits, RecoveryKey};
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::unencrypted(
						crate::RECOVERY_PLAINTEXT_WARNING.into(),
					)
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("bundle")
		};
		let mut tampered = bytes.clone();
		*tampered.last_mut().unwrap() ^= 1;
		assert!(
			core.execute(
				&actor(),
				request(Command::ImportRecoveryBundle {
					bytes: tampered,
					key: RecoveryKey::unencrypted()
				})
			)
			.await
			.is_err()
		);
		let core = core.with_artifact_limits(ArtifactLimits {
			free_reserve_bytes: u64::MAX,
			..ArtifactLimits::default()
		});
		let error = core
			.execute(
				&actor(),
				request(Command::ImportRecoveryBundle {
					bytes,
					key: RecoveryKey::unencrypted(),
				}),
			)
			.await
			.unwrap_err();
		assert_eq!(error.code, "storage.disk_pressure");
		assert!(!directory.path().join("recovery/imports").exists());
	}

	#[tokio::test]
	async fn export_refuses_unknown_transcript_payload_versions() {
		use crate::{RetentionPolicy, WorkingTreeRequest};
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
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
			panic!("Conversation")
		};
		core.store
			.write(async |tx| {
				tx.append_event(jet_store::NewEvent {
					event_id: uuid::Uuid::new_v4(),
					actor: actor().record(),
					recorded_at_unix_ms: 0,
					conversation_id: Some(conversation.conversation_id.0),
					run_id: None,
					kind: "turn.input".into(),
					payload_version: 2,
					payload: r#"{"text":"future payload","turn_id":"future"}"#
						.into(),
					class: jet_store::EventClass::Semantic,
				})
				.await
			})
			.await
			.unwrap();
		assert!(
			core.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::unencrypted(
						crate::RECOVERY_PLAINTEXT_WARNING.into()
					)
					.unwrap(),
				})
			)
			.await
			.is_err()
		);
	}

	#[tokio::test]
	async fn import_rejects_verified_components_with_incompatible_event_content()
	 {
		use crate::{RecoveryKey, RetentionPolicy, WorkingTreeRequest};
		let directory = tempfile::tempdir().unwrap();
		let core = start_core(&directory.path().join("plane.sqlite3")).await;
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
			panic!("Conversation")
		};
		let CommandOutcome::RecoveryBundle(bytes) = core
			.execute(
				&actor(),
				request(Command::ExportRecoveryBundle {
					protection: RecoveryProtection::unencrypted(
						crate::RECOVERY_PLAINTEXT_WARNING.into(),
					)
					.unwrap(),
				}),
			)
			.await
			.unwrap()
		else {
			panic!("bundle")
		};
		for (version, conversation_id, payload) in [
			(
				2,
				Some(conversation.conversation_id.0),
				r#"{"text":"future","turn_id":"x"}"#,
			),
			(
				1,
				Some(conversation.conversation_id.0),
				r#"{"text":false,"turn_id":"x"}"#,
			),
			(1, None, r#"{"text":"orphan","turn_id":"x"}"#),
		] {
			// Fixture construction deliberately makes valid checksums around bad
			// SQLite content; the assertion exercises the public import Command.
			let fixture = tempfile::tempdir().unwrap();
			let path = fixture.path().join("state.sqlite3");
			let mut parts = super::format::decode(&bytes).unwrap();
			std::fs::write(&path, &parts["state.sqlite3"]).unwrap();
			let store = jet_store::Store::open(&path).await.unwrap();
			store
				.write(async |tx| {
					tx.append_event(jet_store::NewEvent {
						event_id: uuid::Uuid::new_v4(),
						actor: actor().record(),
						recorded_at_unix_ms: 0,
						conversation_id,
						run_id: None,
						kind: "turn.input".into(),
						payload_version: version,
						payload: payload.into(),
						class: jet_store::EventClass::Semantic,
					})
					.await
				})
				.await
				.unwrap();
			store.close().await;
			parts.insert("state.sqlite3".into(), std::fs::read(&path).unwrap());
			let malformed = super::format::encode(parts).unwrap();
			assert!(
				core.execute(
					&actor(),
					request(Command::ImportRecoveryBundle {
						bytes: malformed,
						key: RecoveryKey::unencrypted()
					})
				)
				.await
				.is_err()
			);
		}
		assert_eq!(
			std::fs::read_dir(directory.path().join("recovery/imports"))
				.unwrap()
				.count(),
			0
		);
	}
}
