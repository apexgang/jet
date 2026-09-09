//! Transactional staging and serialized native mutations (ADR-0043, ADR-0067).
use crate::{
	Actor, AuditDecision, ClientId, CommandId, CommandOutcome, Core, CoreError,
	ExtensionAction, ExtensionCatalog, ExtensionChange, ExtensionChangeState,
	ExtensionConfirmation, ExtensionHost,
};
use jet_store::{
	EffectKindRecord, EffectSafetyRecord, EffectStateRecord, NewEffect,
	ReadTransaction, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Document {
	change: ExtensionChange,
	confirmation: ExtensionConfirmation,
	client_id: Uuid,
}

async fn admitted_pin(
	core: &Core,
	id: &str,
) -> Result<crate::PinnedCraft, CoreError> {
	if !identifier(id) {
		return Err(invalid());
	}
	let pin = core
		.extension_host
		.as_ref()
		.ok_or_else(unavailable)?
		.pin(id)
		.await?;
	if pin.id != id {
		return Err(invalid());
	}
	core.store
		.read(async |tx| crate::craft_lifecycle::admit(tx, &pin).await)
		.await?;
	if let Some(run_host) = &core.run_host
		&& run_host
			.revoked_craft_digests()
			.await?
			.contains(&pin.sha256)
	{
		return Err(crate::craft_lifecycle::revoked());
	}
	Ok(pin)
}
pub(crate) async fn catalog(
	core: &Core,
	id: &str,
) -> Result<ExtensionCatalog, CoreError> {
	let pin = admitted_pin(core, id).await?;
	let host = core.extension_host.as_ref().ok_or_else(unavailable)?;
	let catalog =
		tokio::time::timeout(Duration::from_secs(30), host.catalog(&pin))
			.await
			.map_err(|_| unavailable())??;
	if catalog.craft_id != id
		|| !identifier(&catalog.harness)
		|| catalog.native_metadata.len() > 65536
		|| serde_json::from_str::<serde_json::Value>(&catalog.native_metadata)
			.is_err()
	{
		return Err(invalid());
	}
	Ok(catalog)
}
pub(crate) async fn inspect(
	core: &Core,
	craft: &str,
	id: &str,
) -> Result<ExtensionCatalog, CoreError> {
	if !extension_identifier(id) {
		return Err(invalid());
	}
	// The exact admitted artifact is also the artifact the host executes.
	let pin = admitted_pin(core, craft).await?;
	let value = tokio::time::timeout(
		Duration::from_secs(30),
		core.extension_host
			.as_ref()
			.ok_or_else(unavailable)?
			.inspect(&pin, id),
	)
	.await
	.map_err(|_| unavailable())??;
	if value.craft_id != craft
		|| !identifier(&value.harness)
		|| value.native_metadata.len() > 65536
		|| serde_json::from_str::<serde_json::Value>(&value.native_metadata)
			.is_err()
	{
		return Err(invalid());
	}
	Ok(value)
}
pub(crate) async fn validate(
	core: &Core,
	confirmation: &ExtensionConfirmation,
) -> Result<(), CoreError> {
	// ASVS 2.2.1, 8.3.1: accept closed lifecycle/scope/trust enums and re-read native consent data.
	if !extension_identifier(&confirmation.extension_id)
		|| confirmation.catalog.native_metadata.len() > 65536
	{
		return Err(invalid());
	}
	if inspect(
		core,
		&confirmation.catalog.craft_id,
		&confirmation.extension_id,
	)
	.await? != confirmation.catalog
	{
		return Err(CoreError::conflict(
			"extension.preview_stale",
			"the native catalog changed; review its metadata and permissions again",
		));
	}
	Ok(())
}
fn extension_identifier(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 4096
		&& !value.chars().any(char::is_control)
}
fn identifier(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 160
		&& value
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
		&& !value.starts_with(['-', '.'])
}
pub(crate) async fn admit(
	tx: &mut WriteTransaction,
	actor: &Actor,
	command_id: CommandId,
	confirmation: ExtensionConfirmation,
	now: i64,
) -> Result<CommandOutcome, CoreError> {
	if !tx
		.unresolved_effects_of(EffectKindRecord::ChangeExtension)
		.await?
		.is_empty()
	{
		return Err(CoreError::conflict(
			"extension.change_pending",
			"a native extension change is already waiting to settle",
		));
	}
	let change_id = Uuid::now_v7();
	let document = Document {
		change: ExtensionChange {
			change_id,
			craft_id: confirmation.catalog.craft_id.clone(),
			extension_id: confirmation.extension_id.clone(),
			action: confirmation.action,
			state: ExtensionChangeState::Staged,
		},
		confirmation,
		client_id: actor.client_id().0,
	};
	save(tx, &document).await?;
	tx.insert_effect(&NewEffect {
		effect_id: change_id,
		command_id: command_id.0,
		run_id: None,
		promotion_id: None,
		terminal_id: None,
		kind: EffectKindRecord::ChangeExtension,
		safety: EffectSafetyRecord::Ambiguous,
	})
	.await?;
	audit(tx, &document, crate::AuditOutcome::Succeeded, now).await?;
	Ok(CommandOutcome::ExtensionChangeQueued { change_id })
}
pub(crate) async fn admit_run(
	tx: &mut ReadTransaction,
) -> Result<(), CoreError> {
	if !tx
		.unresolved_effects_of(EffectKindRecord::ChangeExtension)
		.await?
		.is_empty()
	{
		return Err(CoreError::conflict(
			"extension.change_pending",
			"new Runs wait for the staged native extension change",
		));
	}
	Ok(())
}
pub(crate) async fn query(
	tx: &mut ReadTransaction,
	id: Uuid,
) -> Result<ExtensionChange, CoreError> {
	Ok(load(tx, id).await?.change)
}
async fn load(
	tx: &mut ReadTransaction,
	id: Uuid,
) -> Result<Document, CoreError> {
	let document = tx.extension_change(id).await?.ok_or_else(|| {
		CoreError::not_found(
			"extension.change_not_found",
			"the extension change does not exist",
		)
	})?;
	serde_json::from_str(&document).map_err(|_| invalid())
}
async fn save(
	tx: &mut WriteTransaction,
	document: &Document,
) -> Result<(), CoreError> {
	tx.save_extension_change(
		document.change.change_id,
		&serde_json::to_string(document).map_err(|_| invalid())?,
	)
	.await?;
	Ok(())
}
pub(crate) fn decision(action: ExtensionAction) -> AuditDecision {
	match action {
		ExtensionAction::Install => AuditDecision::ExtensionInstall,
		ExtensionAction::Update => AuditDecision::ExtensionUpdate,
		ExtensionAction::Disable => AuditDecision::ExtensionDisable,
		ExtensionAction::Remove => AuditDecision::ExtensionRemove,
	}
}
async fn audit(
	tx: &mut WriteTransaction,
	document: &Document,
	outcome: crate::AuditOutcome,
	now: i64,
) -> Result<(), CoreError> {
	// ASVS 16.2.1, 16.2.5: attribution and outcome only; never native metadata or configuration.
	crate::audit::record(
		tx,
		&Actor::InteractiveClient {
			client_id: ClientId(document.client_id),
		},
		crate::audit::Decision {
			decision: decision(document.change.action),
			subject: crate::audit::AuditSubject::Extension(
				document.change.change_id,
			),
			outcome,
		},
		now,
	)
	.await
}
fn invalid() -> CoreError {
	CoreError::invalid_input(
		"extension.invalid",
		"the native extension request or catalog is invalid",
	)
}
fn unavailable() -> CoreError {
	CoreError::conflict(
		"extension.unavailable",
		"the responsible Craft cannot manage native extensions",
	)
}

impl Core {
	/// Supply an accepted-Craft adapter before accepting native lifecycle work.
	pub fn with_extension_host(mut self, host: Arc<dyn ExtensionHost>) -> Self {
		self.extension_host = Some(host);
		self
	}
	/// Apply staged changes only after existing managed Runs have finished.
	/// # Errors
	/// Returns persistence or audit errors. An interrupted native mutation is never retried.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "serialize extension mutations with Run launch and recovery"
	)]
	pub async fn perform_extension_changes(&self) -> Result<(), CoreError> {
		let _guard = self.effect_reconciliation.lock().await;
		self.security()
			.await
			.admit(crate::security::SecurityClass::Guarded)?;
		let effects = self
			.store
			.read(async |tx| {
				tx.unresolved_effects_of(EffectKindRecord::ChangeExtension)
					.await
			})
			.await?;
		for effect in effects {
			let mut document = self
				.store
				.read(async |tx| load(tx, effect.effect_id).await)
				.await?;
			if effect.state == EffectStateRecord::Pending {
				// ASVS 15.4.2: admission installs a durable barrier before this check.
				// All native user scopes share the Plane. Retained/orphaned executions also block mutation.
				if self
					.store
					.read(async |tx| {
						Ok::<_, CoreError>(
							!tx.active_execution_ids("").await?.is_empty()
								|| !tx
									.orphaned_executions("")
									.await?
									.is_empty(),
						)
					})
					.await?
				{
					continue;
				}
				self.store
					.write(async |tx| {
						tx.begin_effect_attempt(effect.effect_id).await
					})
					.await?;
				document.change.state =
					if validate(self, &document.confirmation).await.is_err() {
						ExtensionChangeState::Refused
					} else {
						match tokio::time::timeout(
							Duration::from_secs(30),
							async {
								let pin = admitted_pin(
									self,
									&document.confirmation.catalog.craft_id,
								)
								.await?;
								self.extension_host
									.as_ref()
									.ok_or_else(unavailable)?
									.apply(&pin, &document.confirmation)
									.await
							},
						)
						.await
						{
							Ok(Ok(())) => ExtensionChangeState::Applied,
							// Even a failed native process may have changed some files.
							Ok(Err(error))
								if error.code == "extension.refused" =>
							{
								ExtensionChangeState::Refused
							}
							Ok(Err(_)) | Err(_) => {
								ExtensionChangeState::OutcomeUnknown
							}
						}
					};
			} else {
				document.change.state = ExtensionChangeState::OutcomeUnknown;
			}
			self.store
				.write(async |tx| {
					save(tx, &document).await?;
					let state = match document.change.state {
						ExtensionChangeState::Applied => {
							EffectStateRecord::Completed
						}
						ExtensionChangeState::Refused => {
							EffectStateRecord::Failed
						}
						ExtensionChangeState::OutcomeUnknown => {
							EffectStateRecord::OutcomeUnknown
						}
						ExtensionChangeState::Staged => unreachable!(),
					};
					tx.finish_effect(effect.effect_id, state).await?;
					let outcome = match document.change.state {
						ExtensionChangeState::Applied => {
							crate::AuditOutcome::Succeeded
						}
						ExtensionChangeState::Refused => {
							crate::AuditOutcome::Denied
						}
						ExtensionChangeState::OutcomeUnknown => {
							crate::AuditOutcome::Failed
						}
						ExtensionChangeState::Staged => unreachable!(),
					};
					audit(tx, &document, outcome, self.now_unix_ms()).await
				})
				.await?;
			self.run_work.notify_one();
		}
		Ok(())
	}
}
