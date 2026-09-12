//! Admission reserves recovery space without making existing state disposable.
use crate::{Command, Core, CoreError, RunLifecycle, WorkingTreeRequest};
use std::{fs::File, path::PathBuf};

pub(crate) const MINIMUM_RESERVE: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) fn reserve(
	file: &File,
	bytes: u64,
	floor: u64,
) -> Result<(), CoreError> {
	let stat =
		rustix::fs::fstatvfs(file).map_err(crate::artifact::files::io_error)?;
	let capacity = stat.f_blocks.saturating_mul(stat.f_frsize);
	let available = stat.f_bavail.saturating_mul(stat.f_frsize);
	let reserve = MINIMUM_RESERVE.max(capacity.div_ceil(20)).max(floor);
	// ASVS 15.2.2, 16.5.2: leave recovery capacity even for configured large writes.
	if available < reserve || bytes > available - reserve {
		return Err(pressure());
	}
	Ok(())
}

pub(crate) fn pressure() -> CoreError {
	CoreError::unavailable(
		"storage.disk_pressure",
		"new Runs and large writes are paused until disk space recovers",
		"free-space reserve or disposable-storage budget",
	)
}

impl Core {
	pub(crate) async fn check_disk(&self, bytes: u64) -> Result<(), CoreError> {
		self.check_disk_at(self.run_home(), bytes).await
	}

	pub(crate) async fn check_disk_at(
		&self,
		path: PathBuf,
		bytes: u64,
	) -> Result<(), CoreError> {
		let floor = self.artifact_limits.free_reserve_bytes;
		crate::filesystem::blocking(move || {
			let file = rustix::fs::open(
				&path,
				rustix::fs::OFlags::RDONLY
					| rustix::fs::OFlags::DIRECTORY
					| rustix::fs::OFlags::NOFOLLOW
					| rustix::fs::OFlags::CLOEXEC,
				rustix::fs::Mode::empty(),
			)
			.map_err(crate::artifact::files::io_error)?;
			reserve(&File::from(file), bytes, floor)
		})
		.await?
	}
}

pub(crate) fn requires_disk_admission(command: &Command) -> bool {
	match command {
		Command::CreateRun { .. }
		| Command::StartRun { .. }
		| Command::StartNoVisaRun(_)
		| Command::StartVisaRun(_)
		| Command::ForkConversation { .. }
		| Command::HandoffConversation(_)
		| Command::InstallCraft { .. }
		| Command::ChangeExtension { .. }
		| Command::PromoteWorkspace { .. }
		| Command::OpenTerminal { .. }
		| Command::DeliverGit { .. } => true,
		Command::CreateConversation { working_tree, .. }
		| Command::ResumeImportedConversation { working_tree, .. } => {
			matches!(working_tree, WorkingTreeRequest::Workspace { .. })
		}
		Command::TransitionRun { lifecycle, .. } => {
			matches!(lifecycle, RunLifecycle::Starting | RunLifecycle::Active)
		}
		Command::ApplyUserEdit { content, .. } => content.len() >= 128 * 1024,
		Command::AuthorizeApprovalRetry { .. }
		| Command::ReviewRemoteTool { .. }
		| Command::AcknowledgeGitDelivery { .. }
		| Command::RequestUtility { .. }
		| Command::DisableCraft { .. }
		| Command::SubmitReview { .. }
		| Command::SetConversationName { .. }
		| Command::SetRunName { .. }
		| Command::WithdrawTurn { .. }
		| Command::CloseTerminal { .. }
		| Command::SetAutoContinue { .. }
		| Command::CreateSchedule { .. }
		| Command::CancelSchedule { .. }
		| Command::SubmitTurn { .. }
		| Command::ResolveExecution(_)
		| Command::ControlRun { .. }
		| Command::RegisterProject { .. }
		| Command::ImportConversation { .. }
		| Command::SetSetting { .. }
		| Command::ClearSetting { .. }
		| Command::BindAccount { .. }
		| Command::UnbindAccount { .. }
		| Command::BeginAuditEpoch
		| Command::RestoreRecoverySnapshot { .. }
		| Command::PurgeRecoverySnapshots
		| Command::SetPairingGate { .. }
		| Command::OpenPairing { .. }
		| Command::ClaimPairing { .. }
		| Command::ConfirmPairing { .. }
		| Command::SetPairedClientAccess { .. }
		| Command::RevokePairedClient { .. }
		| Command::CompletePairing { .. } => false,
	}
}

impl Core {
	pub(crate) async fn check_effect_disk(
		&self,
		kind: &crate::effect::EffectKind,
	) -> Result<(), CoreError> {
		self.check_disk(0).await?;
		let root = self
			.store
			.read(async |tx| match kind {
				crate::effect::EffectKind::StartRun { run_id } => {
					let Some(record) = tx.run_execution(run_id.0).await? else {
						return Ok(None);
					};
					let plan: crate::LaunchPlan =
						crate::run::state::decode(&record.plan)?;
					Ok(Some(plan.root))
				}
				crate::effect::EffectKind::PromoteWorkspace {
					promotion_id,
				} => {
					let Some(record) = tx.promotion(promotion_id.0).await?
					else {
						return Ok(None);
					};
					let (_, root) = crate::promotion::workspace_and_project(
						tx,
						crate::WorkspaceId(record.workspace_id),
					)
					.await?;
					Ok(Some(root))
				}
				crate::effect::EffectKind::ChangeExtension
				| crate::effect::EffectKind::Utility
				| crate::effect::EffectKind::GitDelivery
				| crate::effect::EffectKind::StartTerminal { .. }
				| crate::effect::EffectKind::CloseTerminal { .. }
				| crate::effect::EffectKind::ResolveExecution
				| crate::effect::EffectKind::ControlRun { .. }
				| crate::effect::EffectKind::InstallCraft => Ok::<_, CoreError>(None),
			})
			.await?;
		if let Some(root) = root {
			self.check_disk_at(root, 0).await?;
		}
		Ok(())
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use crate::test_support::{actor, events, request, start_core};
	use crate::{
		ArtifactLimits, Command, CommandOutcome, RetentionPolicy,
		WorkingTreeRequest,
	};
	use pretty_assertions::assert_eq;

	#[tokio::test]
	async fn pressure_rejects_new_runs_without_poisoning_retries_or_reads() {
		let home = tempfile::tempdir().unwrap();
		let core = start_core(&home.path().join("plane.sqlite3")).await;
		let conversation = conversation(&core).await;
		let command = request(Command::CreateRun {
			conversation_id: conversation,
		});
		let before = events(&core).await;
		let core = core.with_artifact_limits(ArtifactLimits {
			free_reserve_bytes: u64::MAX,
			..Default::default()
		});
		assert_eq!(
			core.execute(&actor(), command.clone())
				.await
				.unwrap_err()
				.code,
			"storage.disk_pressure"
		);
		assert_eq!(events(&core).await, before);
		assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
		let core = core.with_artifact_limits(ArtifactLimits::default());
		let outcome = core.execute(&actor(), command.clone()).await.unwrap();
		let core = core.with_artifact_limits(ArtifactLimits {
			free_reserve_bytes: u64::MAX,
			..Default::default()
		});
		assert_eq!(core.execute(&actor(), command).await.unwrap(), outcome);
	}

	#[tokio::test]
	async fn disposable_budget_reserves_concurrent_uploads_and_releases_abandoned_ones()
	 {
		use crate::{
			ArtifactDescriptor, SettingKey, SettingScope, SettingValue,
		};
		let home = tempfile::tempdir().unwrap();
		let core = start_core(&home.path().join("plane.sqlite3")).await;
		let conversation = conversation(&core).await;
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: conversation,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::StorageDisposableMiB,
				scope: SettingScope::Plane,
				value: SettingValue::Count(1),
			}),
		)
		.await
		.unwrap();
		let descriptor = ArtifactDescriptor {
			sha256: "0".repeat(64),
			size: 1024 * 1024,
		};
		let first = core
			.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
			.await
			.unwrap();
		let other = start_core(&home.path().join("plane.sqlite3")).await;
		let second = other
			.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
			.await;
		assert_eq!(
			second.err().expect("reserved budget").code,
			"storage.disk_pressure"
		);
		drop(first);
		core.begin_artifact_upload(&actor(), run.run_id, descriptor)
			.await
			.unwrap();
		let mut upload = core.begin_artifact_upload(&actor(), run.run_id,
			ArtifactDescriptor {
				sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
				size: 3,
			}).await.unwrap();
		upload.write_chunk(b"abc").await.unwrap();
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::StorageDisposableMiB,
				scope: SettingScope::Plane,
				value: SettingValue::Count(0),
			}),
		)
		.await
		.unwrap();
		assert_eq!(
			core.publish_artifact(&actor(), upload)
				.await
				.unwrap_err()
				.code,
			"storage.disk_pressure"
		);
	}

	#[tokio::test]
	async fn pressure_preserves_published_artifacts_and_collects_only_expired_disposable_data()
	 {
		use crate::test_support::{
			FixedProbe, ManualClock, equipped, start_core_with,
		};
		use crate::{
			ArtifactDescriptor, SettingKey, SettingScope, SettingValue,
		};
		let home = tempfile::tempdir().unwrap();
		let path = home.path().join("plane.sqlite3");
		let clock = ManualClock::at(std::time::SystemTime::now());
		let core =
			start_core_with(&path, clock.clone(), FixedProbe::new(equipped()))
				.await;
		let conversation = conversation(&core).await;
		let CommandOutcome::RunCreated(run) = core
			.execute(
				&actor(),
				request(Command::CreateRun {
					conversation_id: conversation,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("Run")
		};
		let descriptor = ArtifactDescriptor {
			sha256:
				"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
					.into(),
			size: 3,
		};
		let mut upload = core
			.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
			.await
			.unwrap();
		upload.write_chunk(b"abc").await.unwrap();
		core.publish_artifact(&actor(), upload).await.unwrap();
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::StorageDisposableMiB,
				scope: SettingScope::Plane,
				value: SettingValue::Count(0),
			}),
		)
		.await
		.unwrap();
		let cache = home.path().join("cache");
		std::fs::create_dir(&cache).unwrap();
		std::fs::write(cache.join("0".repeat(64)), b"cache").unwrap();
		std::fs::write(cache.join("unrecognized"), b"keep").unwrap();
		let protected = home.path().join("workspaces");
		std::fs::create_dir_all(&protected).unwrap();
		std::fs::write(protected.join("dirty"), b"keep").unwrap();
		std::os::unix::fs::symlink(&protected, cache.join("1".repeat(64)))
			.unwrap();
		std::fs::write(
			home.path().join("artifacts/payloads").join("2".repeat(64)),
			b"orphan",
		)
		.unwrap();
		assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
		core.close().await;
		let core =
			start_core_with(&path, clock.clone(), FixedProbe::new(equipped()))
				.await;
		assert_eq!(
			core.begin_artifact_upload(
				&actor(),
				run.run_id,
				descriptor.clone()
			)
			.await
			.err()
			.expect("budget survived restart")
			.code,
			"storage.disk_pressure"
		);
		let core = core.with_artifact_limits(ArtifactLimits {
			free_reserve_bytes: u64::MAX,
			..Default::default()
		});
		clock.advance(std::time::Duration::from_secs(86401));
		assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 2);
		let mut download =
			core.artifact(&actor(), &descriptor.sha256).await.unwrap();
		assert_eq!(download.read_chunk(3).await.unwrap(), b"abc");
		download.finish().unwrap();
		assert_eq!(std::fs::read(protected.join("dirty")).unwrap(), b"keep");
		assert_eq!(std::fs::read(cache.join("unrecognized")).unwrap(), b"keep");
	}

	async fn conversation(core: &crate::Core) -> crate::ConversationId {
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
		conversation.conversation_id
	}
}
