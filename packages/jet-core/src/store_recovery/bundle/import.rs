//! Fully authenticated input is staged outside the live Plane store.
use super::{RecoveryKey, crypto, format};
use crate::{Actor, CommandOutcome, ConversationId, Core, CoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
	fs,
	io::Write,
	os::unix::fs::{DirBuilderExt, OpenOptionsExt},
	path::PathBuf,
};
use uuid::Uuid;

/// A Conversation retained as a non-authoritative Recovered copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredConversation {
	/// Provenance only; never an authority claim.
	pub source: ConversationId,
	/// Fresh identity in the Recovered snapshot.
	pub recovered: ConversationId,
}
/// A verified, staged import. It cannot be used as an authoritative Store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredBundle {
	/// Identity of this import, independent from the source bundle.
	pub bundle_id: Uuid,
	/// Owner-only archive directory, outside live store and snapshot restoration.
	pub directory: PathBuf,
	/// Fresh identities of the imported Conversations.
	pub conversations: Vec<RecoveredConversation>,
	/// Number of schedules retained as disabled metadata.
	pub disabled_schedules: u64,
	/// Verified immutable Artifact declarations.
	pub artifacts: Vec<crate::ArtifactDescriptor>,
}
impl Core {
	pub(crate) async fn import_recovery_bundle(
		&self,
		actor: &Actor,
		bytes: Vec<u8>,
		key: RecoveryKey,
		command_id: crate::CommandId,
	) -> Result<CommandOutcome, CoreError> {
		if let crate::RecoveryMode::ReadOnly(reason) = self.recovery_mode() {
			return Err(crate::store_recovery::read_only(reason));
		}
		// Bind retries to the bundle, excluding ephemeral keys and caller-supplied
		// request encoding, either of which could disclose a password verifier.
		let request_digest: [u8; 32] = Sha256::digest(&bytes).into();
		let id = Uuid::new_v5(&actor.client_id().0, command_id.0.as_bytes());
		let root = self.run_home().join("recovery").join("imports");
		let target = root.join(format!("recovered-{id}"));
		let receipt_path = target.join("recovered.json");
		if receipt_path.exists() {
			let receipt = crate::filesystem::blocking(move || {
				let mut remaining = format::MAX_BYTES;
				let bytes = super::read_bounded(
					fs::File::open(receipt_path)
						.map_err(crate::artifact::files::io_error)?,
					&mut remaining,
				)?;
				serde_json::from_slice::<ImportReceipt>(&bytes)
					.map_err(|_| crypto::invalid())
			})
			.await??;
			if receipt.request_digest != request_digest {
				return Err(CoreError::conflict(
					"recovery.command_reused",
					"Command identity was already used for another Recovery import",
				));
			}
			if receipt.result.directory != target
				|| receipt.result.bundle_id != id
			{
				return Err(crypto::invalid());
			}
			return Ok(CommandOutcome::RecoveredBundle(receipt.result));
		}
		self.security
			.read()
			.await
			.admit(crate::security::SecurityClass::Guarded)?;
		// ASVS 11.2.5: consume and authenticate the final stream chunk before
		// any plaintext is staged. Wrong keys and truncation leave nothing.
		let parts = crate::filesystem::blocking(move || {
			format::decode(&crypto::decrypt(&bytes, key)?)
		})
		.await??;
		let artifacts = format::artifacts(&parts);
		// Reserve the payload plus SQLite journal/VACUUM scratch and metadata.
		let staged_bytes: u64 =
			parts.values().map(|part| part.len() as u64).sum();
		self.check_disk(
			staged_bytes
				+ 2 * parts["state.sqlite3"].len() as u64
				+ 1024 * 1024,
		)
		.await?;
		let directory = crate::filesystem::blocking(move || {
			fs::DirBuilder::new()
				.recursive(true)
				.mode(0o700)
				.create(&root)?;
			let directory = tempfile::Builder::new()
				.prefix(".pending-")
				.tempdir_in(&root)?;
			fs::DirBuilder::new()
				.mode(0o700)
				.create(directory.path().join("artifacts"))?;
			for (name, bytes) in parts {
				let mut file = fs::OpenOptions::new()
					.write(true)
					.create_new(true)
					.mode(0o600)
					.open(directory.path().join(name))?;
				file.write_all(&bytes)?;
				file.sync_all()?;
			}
			Ok::<_, std::io::Error>(directory)
		})
		.await?
		.map_err(crate::artifact::files::io_error)?;
		let snapshot = self
			.store
			.recover_portable_snapshot(&directory.path().join("state.sqlite3"))
			.await?;
		let expected = jet_store::Store::portable_artifacts(
			&directory.path().join("state.sqlite3"),
		)
		.await?;
		let supplied: Vec<_> = artifacts
			.iter()
			.map(|a| (a.sha256.clone(), a.size))
			.collect();
		if expected != supplied {
			return Err(crypto::invalid());
		}
		let recovered = RecoveredBundle {
			bundle_id: id,
			directory: target.clone(),
			conversations: snapshot
				.conversations
				.into_iter()
				.map(|(source, recovered)| RecoveredConversation {
					source: ConversationId(source),
					recovered: ConversationId(recovered),
				})
				.collect(),
			disabled_schedules: snapshot.disabled_schedules,
			artifacts,
		};
		self.authorize_bundle_action(
			actor,
			crate::AuditDecision::RecoveryImportAuthorized,
		)
		.await?;
		let metadata = serde_json::to_vec(&ImportReceipt {
			request_digest,
			result: recovered.clone(),
		})
		.map_err(|_| crypto::invalid())?;
		crate::filesystem::blocking(move || {
			let mut file = fs::OpenOptions::new()
				.create_new(true)
				.write(true)
				.mode(0o600)
				.open(directory.path().join("recovered.json"))?;
			file.write_all(&metadata)?;
			file.sync_all()?;
			fs::File::open(directory.path().join("state.sqlite3"))?
				.sync_all()?;
			fs::File::open(directory.path().join("artifacts"))?.sync_all()?;
			fs::File::open(directory.path())?.sync_all()?;
			fs::rename(directory.path(), &target)?;
			fs::File::open(target.parent().expect("import parent"))?
				.sync_all()?;
			Ok::<_, std::io::Error>(())
		})
		.await?
		.map_err(crate::artifact::files::io_error)?;
		Ok(CommandOutcome::RecoveredBundle(recovered))
	}
}

// Published in the same atomic directory rename as the recovered content.
// The receipt survives restart without coordinating a second database commit.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportReceipt {
	request_digest: [u8; 32],
	result: RecoveredBundle,
}
