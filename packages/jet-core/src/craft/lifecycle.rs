//! Craft admission and process lifetime, independent of individual Harnesses.
use crate::{Actor, CommandOutcome, Core, CoreError, PinnedCraft, RunId};
use jet_store::{ReadTransaction, WriteTransaction};

/// How an interactive disable handles already accepted Runs.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CraftDisableMode {
	/// Block new Runs and let pinned Runs finish.
	Wait,
	/// Stop the Craft while leaving its Harnesses under their helpers.
	Force,
}

pub(crate) async fn admit(
	tx: &mut ReadTransaction,
	pin: &PinnedCraft,
) -> Result<(), CoreError> {
	if tx.craft_revoked(&pin.sha256).await? {
		return Err(revoked());
	}
	if tx.craft_disabled(&pin.id).await?.is_some() {
		return Err(disabled());
	}
	Ok(())
}
pub(crate) fn revoked() -> CoreError {
	CoreError::conflict(
		"craft.revoked",
		"the Craft digest was revoked by signed Jet release metadata",
	)
}
fn disabled() -> CoreError {
	CoreError::conflict("craft.disabled", "the Craft is disabled for new Runs")
}
pub(crate) async fn record(
	tx: &mut WriteTransaction,
	actor: &Actor,
	id: String,
	mode: CraftDisableMode,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	if id.is_empty()
		|| id.len() > 80
		|| !id
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
	{
		return Err(CoreError::invalid_input(
			"craft.invalid_id",
			"the Craft identity is invalid",
		));
	}
	tx.disable_craft(
		&id,
		match mode {
			CraftDisableMode::Wait => jet_store::CraftDisableMode::Wait,
			CraftDisableMode::Force => jet_store::CraftDisableMode::Force,
		},
	)
	.await?;
	let mode = if tx.craft_disabled(&id).await?
		== Some(jet_store::CraftDisableMode::Force)
	{
		CraftDisableMode::Force
	} else {
		CraftDisableMode::Wait
	};
	crate::audit::record(
		tx,
		actor,
		crate::audit::Decision::succeeded(
			crate::AuditDecision::CraftDisabled,
			crate::audit::AuditSubject::Craft(id.clone()),
		),
		now,
	)
	.await?;
	Ok(CommandOutcome::CraftDisabled { craft_id: id, mode })
}
impl Core {
	/// Applies committed disable barriers without signalling Harness processes.
	/// # Errors
	/// Returns a store or host error; a failed stop retains its durable barrier.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "Craft stops serialize with launch and recovery Effects"
	)]
	pub async fn reconcile_crafts(&self) -> Result<(), CoreError> {
		let Some(host) = &self.run_host else {
			return Ok(());
		};
		let _gate = self.effect_reconciliation.lock().await;
		let signed = host.revoked_craft_digests().await?;
		self.store
			.write(async |tx| {
				for digest in &signed {
					crate::audit::record_craft_revocation(
						tx,
						digest,
						self.now_unix_ms(),
					)
					.await?;
				}
				Ok::<_, CoreError>(())
			})
			.await?;
		let revoked = self
			.store
			.read(async |tx| tx.craft_revocations().await)
			.await?;
		let disables = self
			.store
			.read(async |tx| tx.craft_disables().await)
			.await?;
		let mut active = Vec::new();
		let mut stopped = revoked.clone();
		let mut affected = Vec::new();
		let mut after = String::new();
		loop {
			let page = self
				.store
				.read(async |tx| tx.active_execution_ids(&after).await)
				.await?;
			let Some(last) = page.last() else {
				break;
			};
			after = last.to_string();
			for id in page {
				let record = self
					.store
					.read(async |tx| tx.run_execution(id).await)
					.await?;
				if let Some(record) = record {
					let plan: crate::LaunchPlan =
						crate::run::state::decode(&record.plan)?;
					let craft_id = host.craft_id(&plan.craft)?;
					if revoked.contains(&plan.craft.sha256)
						|| disables.iter().any(|(id, mode)| {
							*mode == jet_store::CraftDisableMode::Force
								&& *id == craft_id
						}) {
						stopped.push(plan.craft.sha256.clone());
						affected.push(RunId(id));
					}
					active.push(plan.craft);
				}
			}
		}
		let force_disabled = disables
			.into_iter()
			.filter_map(|(id, mode)| {
				(mode == jet_store::CraftDisableMode::Force).then_some(id)
			})
			.collect();
		host.maintain_crafts(active, stopped, force_disabled)
			.await?;
		for id in affected {
			self.mark_orphan(id).await?;
			self.observe_run(id, crate::run::state::Observation::Disconnected)
				.await?;
		}
		Ok(())
	}
	pub(crate) async fn craft_recovery_allowed(
		&self,
		pin: &PinnedCraft,
	) -> Result<(), CoreError> {
		if self
			.store
			.read(async |tx| tx.craft_revoked(&pin.sha256).await)
			.await?
		{
			return Err(revoked());
		}
		let id = self.run_host.as_ref().ok_or_else(disabled)?.craft_id(pin)?;
		if self
			.store
			.read(async |tx| tx.craft_disabled(&id).await)
			.await? == Some(jet_store::CraftDisableMode::Force)
		{
			return Err(disabled());
		}
		Ok(())
	}
}
