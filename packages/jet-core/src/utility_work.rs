//! Commit-before-inference Utility Effects, with no retries after uncertainty.
use crate::{
	AccountBinding, AccountBindingId, Actor, CommandId, CommandOutcome, Core,
	CoreError, CredentialState, PlaneId, SettingKey, SettingScope,
	SettingValue, UtilityHost, UtilityJob, UtilityOutcome, UtilityPolicy,
	UtilityRequest,
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
	job: UtilityJob,
	request: UtilityRequest,
}

pub(crate) async fn admit(
	tx: &mut WriteTransaction,
	_actor: &Actor,
	command_id: CommandId,
	request: UtilityRequest,
) -> Result<CommandOutcome, CoreError> {
	if let UtilityRequest::Autodelete { prompt } = &request
		&& (prompt.trim().is_empty() || prompt.len() > 4096)
	{
		return Err(CoreError::invalid_input(
			"utility.input_limit",
			"rule prompts contain 1 to 4096 bytes",
		));
	}
	if tx
		.unresolved_effects_of(EffectKindRecord::Utility)
		.await?
		.len() >= 32
	{
		return Err(CoreError::conflict(
			"utility.queue_full",
			"the Plane already has 32 pending Utility requests",
		));
	}
	let scope = scope(tx, &request).await?;
	let (binding_id, policy) = policy(tx, &request, scope).await?;
	let provider = match binding_id {
		Some(id) => tx
			.account_binding(id.0)
			.await?
			.map(|b| crate::ProviderId(b.provider)),
		None => None,
	};
	let job = UtilityJob {
		job_id: Uuid::now_v7(),
		plane_id: PlaneId(tx.plane().await?.plane_id),
		purpose: match request {
			UtilityRequest::Naming { .. } => crate::UtilityPurpose::Naming,
			UtilityRequest::GitText { .. } => crate::UtilityPurpose::GitText,
			UtilityRequest::Autodelete { .. } => {
				crate::UtilityPurpose::Autodelete
			}
		},
		provider,
		binding_id,
		model: None,
		policy,
		outcome: UtilityOutcome::Pending,
	};
	let id = job.job_id;
	let run_id = match &request {
		UtilityRequest::Naming { run_id }
		| UtilityRequest::GitText { run_id, .. } => Some(run_id.0),
		UtilityRequest::Autodelete { .. } => None,
	};
	save(tx, &Document { job, request }).await?;
	tx.insert_effect(&NewEffect {
		effect_id: id,
		command_id: command_id.0,
		run_id,
		promotion_id: None,
		terminal_id: None,
		kind: EffectKindRecord::Utility,
		safety: EffectSafetyRecord::Ambiguous,
	})
	.await?;
	Ok(CommandOutcome::UtilityQueued { job_id: id })
}
async fn scope(
	tx: &mut ReadTransaction,
	request: &UtilityRequest,
) -> Result<SettingScope, CoreError> {
	match request {
		UtilityRequest::Autodelete { .. } => Ok(SettingScope::Plane),
		UtilityRequest::Naming { run_id }
		| UtilityRequest::GitText { run_id, .. } => {
			let run = tx
				.run(run_id.0)
				.await?
				.ok_or_else(|| unavailable("utility.source_unavailable"))?;
			Ok(SettingScope::Conversation {
				conversation_id: crate::ConversationId(run.conversation_id),
			})
		}
	}
}
async fn policy(
	tx: &mut ReadTransaction,
	request: &UtilityRequest,
	scope: SettingScope,
) -> Result<(Option<AccountBindingId>, UtilityPolicy), CoreError> {
	let stored = tx.settings_for_scope(scope.record()).await?;
	let purpose_key = match request {
		UtilityRequest::Naming { .. } => SettingKey::UtilityAutomaticNaming,
		UtilityRequest::GitText { .. } => SettingKey::UtilityGitText,
		UtilityRequest::Autodelete { .. } => {
			SettingKey::UtilityAutodeleteCompilation
		}
	};
	let values = crate::setting::resolve(
		&[
			SettingKey::UtilityAccountBinding,
			purpose_key,
			SettingKey::UtilityContentConsent,
		],
		&stored,
	);
	let id = match &values[0].value {
		SettingValue::Text(value) => {
			Uuid::parse_str(value).ok().map(AccountBindingId)
		}
		_ => None,
	};
	let consent = id.is_some_and(|id| {
		values[2].value == SettingValue::Text(id.0.to_string())
	});
	Ok((
		id,
		UtilityPolicy {
			version: 1,
			enabled: values[1].value == SettingValue::Flag(true),
			cross_provider_consent: consent,
		},
	))
}
pub(crate) async fn query(
	tx: &mut ReadTransaction,
	id: Uuid,
) -> Result<UtilityJob, CoreError> {
	Ok(load(tx, id).await?.job)
}
async fn load(
	tx: &mut ReadTransaction,
	id: Uuid,
) -> Result<Document, CoreError> {
	let json = tx.utility_job(id).await?.ok_or_else(|| {
		CoreError::not_found(
			"utility.not_found",
			"the Utility job does not exist",
		)
	})?;
	serde_json::from_str(&json).map_err(|_| invalid())
}
async fn save(
	tx: &mut WriteTransaction,
	doc: &Document,
) -> Result<(), CoreError> {
	tx.save_utility_job(
		doc.job.job_id,
		&serde_json::to_string(doc).map_err(|_| invalid())?,
	)
	.await?;
	Ok(())
}
fn invalid() -> CoreError {
	CoreError::internal(
		"utility.invalid_state",
		"the Utility document is invalid",
	)
}
pub(crate) fn unavailable(code: &'static str) -> CoreError {
	CoreError::conflict(
		code,
		"Utility inference is unavailable under the recorded policy",
	)
}
impl Core {
	/// Install the trusted inference Adapter before accepting Utility work.
	pub fn with_utility_host(mut self, host: Arc<dyn UtilityHost>) -> Self {
		self.utility_host = Some(host);
		self
	}
	/// Settle committed Utility Effects. An interrupted attempt is never repeated.
	/// # Errors
	/// Returns a persistence error without claiming that an uncommitted result exists.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "serialize Utility claims across the single inference attempt; releasing the guard would let another sweep treat live work as interrupted"
	)]
	pub async fn perform_utilities(&self) -> Result<(), CoreError> {
		let Ok(_guard) = self.utility_work.try_lock() else {
			return Ok(());
		};
		let effects = self
			.store
			.read(async |tx| {
				tx.unresolved_effects_of(EffectKindRecord::Utility).await
			})
			.await?;
		for effect in effects {
			let mut doc = self
				.store
				.read(async |tx| load(tx, effect.effect_id).await)
				.await?;
			let answer = if effect.state == EffectStateRecord::Pending {
				self.store
					.write(async |tx| {
						tx.begin_effect_attempt(effect.effect_id).await
					})
					.await?;
				match tokio::time::timeout(
					Duration::from_secs(30),
					self.infer_utility(&mut doc),
				)
				.await
				{
					Ok(answer) => answer,
					Err(_) => Err(unavailable("utility.timeout")),
				}
			} else {
				Err(unavailable("utility.interrupted"))
			};
			doc.job.outcome = match answer {
				Ok(outcome) => outcome,
				Err(error) => {
					self.utility_fallback(&doc.request, error.code).await
				}
			};
			self.store
				.write(async |tx| {
					save(tx, &doc).await?;
					tx.finish_effect(
						effect.effect_id,
						if effect.state == EffectStateRecord::InFlight {
							EffectStateRecord::OutcomeUnknown
						} else {
							EffectStateRecord::Completed
						},
					)
					.await?;
					Ok::<_, CoreError>(())
				})
				.await?;
		}
		Ok(())
	}
	/// Wait for committed Utility work; notifications coalesce safely.
	pub async fn wait_for_utility_work(&self) {
		self.utility_wake.notified().await;
	}
	async fn utility_binding(
		&self,
		doc: &Document,
	) -> Result<AccountBinding, CoreError> {
		self.security()
			.await
			.admit(crate::security::SecurityClass::Guarded)?;
		let capabilities = self.observe_capabilities().await;
		self.store
			.read(async |tx| {
				let scope = scope(tx, &doc.request).await?;
				if policy(tx, &doc.request, scope).await?
					!= (doc.job.binding_id, doc.job.policy.clone())
				{
					return Err(unavailable("utility.policy_changed"));
				}
				let id = doc.job.binding_id.ok_or_else(|| {
					unavailable("utility.binding_unavailable")
				})?;
				let binding: AccountBinding = tx
					.account_binding(id.0)
					.await?
					.ok_or_else(|| unavailable("utility.binding_unavailable"))?
					.into();
				if Some(&binding.provider) != doc.job.provider.as_ref() {
					return Err(unavailable("utility.binding_unavailable"));
				}
				let credential = CredentialState::of(
					&binding.credential_reference,
					capabilities.credential_store,
					tx.plane().await?.daemon_starts,
				);
				if !matches!(
					credential,
					CredentialState::Resolvable
						| CredentialState::ResolvedAtUse
				) {
					return Err(unavailable("utility.credential_unavailable"));
				}
				Ok(binding)
			})
			.await
	}
	async fn infer_utility(
		&self,
		doc: &mut Document,
	) -> Result<UtilityOutcome, CoreError> {
		if !doc.job.policy.enabled {
			return Err(unavailable("utility.disabled"));
		}
		let binding = self.utility_binding(doc).await?;
		// Existing Run pins do not establish an authoritative Provider identity.
		// Require persistent disclosure for unknown provenance as well as a known cross-Provider send.
		if !matches!(doc.request, UtilityRequest::Autodelete { .. })
			&& !doc.job.policy.cross_provider_consent
		{
			return Err(unavailable("utility.consent_required"));
		}
		let host = self
			.utility_host
			.as_ref()
			.ok_or_else(|| unavailable("utility.host_unavailable"))?;
		let model = host
			.select(&binding)
			.await
			.map_err(|_| unavailable("utility.model_unavailable"))?;
		if model.name.is_empty()
			|| model.name.len() > 128
			|| model.name.chars().any(char::is_control)
			|| model.adapter_state.len() > 16384
		{
			return Err(unavailable("utility.model_invalid"));
		}
		doc.job.model = Some(model.name.clone());
		self.store.write(async |tx| save(tx, doc).await).await?;
		let input = self.utility_input(&doc.request).await?;
		if self.utility_binding(doc).await? != binding {
			return Err(unavailable("utility.binding_changed"));
		}
		let reply = host
			.infer(&binding, &model, &input)
			.await
			.map_err(|_| unavailable("utility.inference_failed"))?;
		if reply.model != model.name || reply.output.len() > 8192 {
			return Err(unavailable("utility.output_invalid"));
		}
		crate::utility_output::validate(&input, &reply.output)
	}
}
