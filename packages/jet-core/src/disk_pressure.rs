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
		rustix::fs::fstatvfs(file).map_err(crate::artifact_files::io_error)?;
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
			.map_err(crate::artifact_files::io_error)?;
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
						crate::run_state::decode(&record.plan)?;
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
