//! Utility requests and results contain data, never Commands (ADR-0099).
pub(crate) mod host;
pub(crate) mod input;
pub(crate) mod output;
pub(crate) mod work;

use crate::{AccountBindingId, PlaneId, ProviderId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Closed set of Utility purposes, used for durable attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UtilityPurpose {
	/// Conversation or Run naming.
	Naming,
	/// Git commit or pull-request text.
	GitText,
	/// Unapproved natural-language rule compilation.
	Autodelete,
}

/// One purpose-specific request for Jet-owned inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityRequest {
	/// Suggest a Run name using only its title and opening input.
	Naming {
		/// Owning Run on this Plane.
		run_id: crate::RunId,
	},
	/// Draft Git commit or pull-request text from one retained turn.
	GitText {
		/// Owning Run.
		run_id: crate::RunId,
		/// Completed or interrupted turn, starting at one.
		turn: u32,
	},
	/// Compile only this rule prompt into an unapproved draft.
	Autodelete {
		/// Rule prompt, bounded to 4096 UTF-8 bytes.
		prompt: String,
	},
}
/// The exact policy used by a job. Version 1 means smallest suitable Model,
/// minimum reasoning, one inference request, no tools, and bounded input/output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtilityPolicy {
	/// Version of the allowlists and limits.
	pub version: u32,
	/// Whether the purpose was enabled.
	pub enabled: bool,
	/// Persistent consent to send content outside its originating Provider.
	pub cross_provider_consent: bool,
}
/// A Utility result carries no execution or deletion authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityOutcome {
	/// Durable admission, awaiting a single inference attempt.
	Pending,
	/// A draft inactivity predicate. It cannot execute until reviewed separately.
	Draft {
		/// Whole days of inactivity, from 1 through 36500.
		inactive_days: u32,
	},
	/// Validated text, or a deterministic local fallback. Never executed.
	Text {
		/// Name or Git subject.
		text: String,
		/// Git body; empty for naming.
		body: String,
		/// Content-free fallback explanation; absent for inference.
		fallback_reason: Option<String>,
	},
	/// Compilation did not produce a usable draft.
	Refused {
		/// Stable, content-free reason.
		reason: String,
	},
}
/// Durable Utility result and its complete routing and policy attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtilityJob {
	/// Plane-assigned request identity.
	pub job_id: Uuid,
	/// The only Plane authorized to execute this request.
	pub plane_id: PlaneId,
	/// Purpose, independent of the generated data.
	pub purpose: UtilityPurpose,
	/// Exact selected Provider; absent when no usable binding was configured.
	pub provider: Option<ProviderId>,
	/// Explicit selected binding, including one that later became unavailable.
	pub binding_id: Option<AccountBindingId>,
	/// Exact selected Model; absent when selection was unavailable.
	pub model: Option<String>,
	/// Policy at admission and the consent it recorded.
	pub policy: UtilityPolicy,
	/// Validated data or a closed failure.
	pub outcome: UtilityOutcome,
}

#[cfg(test)]
pub(crate) mod tests {
	use crate::test_support::{actor, request, start_core};
	use crate::{
		Command, CommandOutcome, Query, QueryResult, UtilityOutcome,
		UtilityRequest,
	};
	use pretty_assertions::assert_eq;

	#[tokio::test]
	async fn unavailable_autodelete_compilation_is_a_durable_refusal() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let core = start_core(&path).await;
		let command = request(Command::RequestUtility {
			request: UtilityRequest::Autodelete {
				prompt: "Forget inactive Conversations after 90 days".into(),
			},
		});
		let outcome = core.execute(&actor(), command.clone()).await.unwrap();
		let CommandOutcome::UtilityQueued { job_id } = outcome else {
			panic!("expected Utility admission")
		};
		core.perform_utilities().await.unwrap();
		assert_eq!(core.execute(&actor(), command).await.unwrap(), outcome);
		core.close().await;
		let core = start_core(&path).await;
		let QueryResult::Utility(job) = core
			.query(&actor(), Query::Utility { job_id })
			.await
			.unwrap()
		else {
			panic!("expected Utility result")
		};
		assert_eq!(
			job.outcome,
			UtilityOutcome::Refused {
				reason: "utility.disabled".into()
			}
		);
	}

	use crate::{
		AccountBinding, Core, CoreError, CredentialSource, ProviderId,
		RunFuture, SettingKey, SettingScope, SettingValue, UtilityHost,
		UtilityInput, UtilityModel, UtilityReply,
	};
	use std::sync::{Arc, Mutex};

	#[derive(Debug)]
	pub(crate) struct Inference {
		pub(crate) calls: Mutex<Vec<UtilityInput>>,
		pub(crate) output: String,
	}
	impl UtilityHost for Inference {
		fn select<'a>(
			&'a self,
			_binding: &'a AccountBinding,
		) -> RunFuture<'a, Result<UtilityModel, CoreError>> {
			Box::pin(async {
				Ok(UtilityModel {
					name: "small-model-v1".into(),
					adapter_state: String::new(),
				})
			})
		}
		fn infer<'a>(
			&'a self,
			_binding: &'a AccountBinding,
			model: &'a UtilityModel,
			input: &'a UtilityInput,
		) -> RunFuture<'a, Result<UtilityReply, CoreError>> {
			self.calls.lock().unwrap().push(input.clone());
			Box::pin(async move {
				Ok(UtilityReply {
					model: model.name.clone(),
					output: self.output.as_bytes().to_vec(),
				})
			})
		}
	}
	pub(crate) async fn setting(
		core: &Core,
		key: SettingKey,
		value: SettingValue,
	) {
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key,
				scope: SettingScope::Plane,
				value,
			}),
		)
		.await
		.unwrap();
	}
	pub(crate) async fn configured(core: &Core) -> AccountBinding {
		let CommandOutcome::AccountBound(binding) = core
			.execute(
				&actor(),
				request(Command::BindAccount {
					provider: ProviderId("anthropic".into()),
					label: "Utility".into(),
					provider_account: None,
					credential_source: CredentialSource::HarnessNative,
				}),
			)
			.await
			.unwrap()
		else {
			panic!("binding")
		};
		setting(
			core,
			SettingKey::UtilityAccountBinding,
			SettingValue::Text(binding.binding_id.0.to_string()),
		)
		.await;
		setting(
			core,
			SettingKey::UtilityAutodeleteCompilation,
			SettingValue::Flag(true),
		)
		.await;
		binding
	}
	pub(crate) async fn generate(
		core: &Core,
		input: UtilityRequest,
	) -> crate::UtilityJob {
		let CommandOutcome::UtilityQueued { job_id } = core
			.execute(
				&actor(),
				request(Command::RequestUtility { request: input }),
			)
			.await
			.unwrap()
		else {
			panic!("admission")
		};
		core.perform_utilities().await.unwrap();
		let QueryResult::Utility(job) = core
			.query(&actor(), Query::Utility { job_id })
			.await
			.unwrap()
		else {
			panic!("Utility result")
		};
		job
	}
	#[tokio::test]
	async fn an_autodelete_draft_records_its_exact_binding_model_and_policy() {
		let dir = tempfile::tempdir().unwrap();
		let peer = Arc::new(Inference {
			calls: Mutex::default(),
			output: r#"{"inactive_days":90}"#.into(),
		});
		let core = start_core(&dir.path().join("plane.sqlite3"))
			.await
			.with_utility_host(peer.clone());
		let binding = configured(&core).await;
		let job = generate(
			&core,
			UtilityRequest::Autodelete {
				prompt: "Inactive for 90 days".into(),
			},
		)
		.await;
		assert_eq!(
			(
				job.provider,
				job.binding_id,
				job.model,
				job.policy,
				job.outcome
			),
			(
				Some(binding.provider),
				Some(binding.binding_id),
				Some("small-model-v1".into()),
				crate::UtilityPolicy {
					version: 1,
					enabled: true,
					cross_provider_consent: false
				},
				UtilityOutcome::Draft { inactive_days: 90 },
			)
		);
		assert_eq!(
			*peer.calls.lock().unwrap(),
			vec![UtilityInput::Autodelete {
				prompt: "Inactive for 90 days".into()
			}]
		);
	}

	#[tokio::test]
	async fn naming_requires_persistent_content_consent_and_sends_only_its_allowlist()
	 {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let peer = Arc::new(Inference {
			calls: Mutex::default(),
			output: r#"{"text":"Fix naming"}"#.into(),
		});
		let core = start_core(&path).await.with_utility_host(peer.clone());
		let binding = configured(&core).await;
		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: crate::RetentionPolicy::Retain,
					working_tree: crate::WorkingTreeRequest::NoProject,
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
		let input = UtilityRequest::Naming { run_id: run.run_id };
		let job = generate(&core, input.clone()).await;
		assert_eq!(
			job.outcome,
			UtilityOutcome::Text {
				text: run.name.value.clone(),
				body: String::new(),
				fallback_reason: Some("utility.consent_required".into())
			}
		);
		assert!(peer.calls.lock().unwrap().is_empty());
		setting(
			&core,
			SettingKey::UtilityContentConsent,
			SettingValue::Text(binding.binding_id.0.to_string()),
		)
		.await;
		core.close().await;
		let core = start_core(&path).await.with_utility_host(peer.clone());
		let job = generate(&core, input).await;
		assert_eq!(
			job.outcome,
			UtilityOutcome::Text {
				text: "Fix naming".into(),
				body: String::new(),
				fallback_reason: None
			}
		);
		assert_eq!(
			*peer.calls.lock().unwrap(),
			vec![UtilityInput::Naming {
				title: run.name.value,
				opening_context: String::new()
			}]
		);
	}

	#[tokio::test]
	async fn invalid_or_oversized_rule_outputs_never_authorize_deletion() {
		for output in [
			"{\"inactive_days\":0}".into(),
			"{\"inactive_days\":36501}".into(),
			"{\"inactive_days\":null}".into(),
			"{\"inactive_days\":90,\"command\":\"delete_conversation\"}".into(),
			"{\"inactive_days\":90,\"inactive_days\":1}".into(),
			format!("{}{{\"inactive_days\":90}}", " ".repeat(8192)),
		] {
			let dir = tempfile::tempdir().unwrap();
			let peer = Arc::new(Inference {
				calls: Mutex::default(),
				output,
			});
			let core = start_core(&dir.path().join("plane.sqlite3"))
				.await
				.with_utility_host(peer);
			configured(&core).await;
			let job = generate(
				&core,
				UtilityRequest::Autodelete {
					prompt: "Inactive for 90 days".into(),
				},
			)
			.await;
			assert_eq!(
				job.outcome,
				UtilityOutcome::Refused {
					reason: "utility.output_invalid".into()
				}
			);
		}
	}
	#[derive(Debug)]
	enum PauseAt {
		Selection,
		Inference,
	}
	#[derive(Debug)]
	struct PausedInference {
		at: PauseAt,
		entered: tokio::sync::Notify,
		resume: tokio::sync::Notify,
		calls: Mutex<usize>,
	}
	impl PausedInference {
		fn new(at: PauseAt) -> Self {
			Self {
				at,
				entered: tokio::sync::Notify::new(),
				resume: tokio::sync::Notify::new(),
				calls: Mutex::new(0),
			}
		}
	}
	impl UtilityHost for PausedInference {
		fn select<'a>(
			&'a self,
			_: &'a AccountBinding,
		) -> RunFuture<'a, Result<UtilityModel, CoreError>> {
			Box::pin(async move {
				if matches!(self.at, PauseAt::Selection) {
					self.entered.notify_one();
					self.resume.notified().await;
				}
				Ok(UtilityModel {
					name: "small-model-v1".into(),
					adapter_state: String::new(),
				})
			})
		}
		fn infer<'a>(
			&'a self,
			_: &'a AccountBinding,
			model: &'a UtilityModel,
			_: &'a UtilityInput,
		) -> RunFuture<'a, Result<UtilityReply, CoreError>> {
			Box::pin(async move {
				*self.calls.lock().unwrap() += 1;
				if matches!(self.at, PauseAt::Inference) {
					self.entered.notify_one();
					self.resume.notified().await;
				}
				Ok(UtilityReply {
					model: model.name.clone(),
					output: br#"{"inactive_days":90}"#.to_vec(),
				})
			})
		}
	}
	async fn queue_rule(core: &Core) -> uuid::Uuid {
		let CommandOutcome::UtilityQueued { job_id } = core
			.execute(
				&actor(),
				request(Command::RequestUtility {
					request: UtilityRequest::Autodelete {
						prompt: "Inactive for 90 days".into(),
					},
				}),
			)
			.await
			.unwrap()
		else {
			panic!("admission")
		};
		job_id
	}
	#[tokio::test]
	async fn a_policy_revoked_during_selection_is_checked_before_content_is_sent()
	 {
		let dir = tempfile::tempdir().unwrap();
		let peer = Arc::new(PausedInference::new(PauseAt::Selection));
		let core = start_core(&dir.path().join("plane.sqlite3"))
			.await
			.with_utility_host(peer.clone());
		configured(&core).await;
		let id = queue_rule(&core).await;
		let work = core.perform_utilities();
		tokio::pin!(work);
		tokio::select! { () = peer.entered.notified() => {}, _ = &mut work => panic!("selection must pause") }
		setting(
			&core,
			SettingKey::UtilityAutodeleteCompilation,
			SettingValue::Flag(false),
		)
		.await;
		peer.resume.notify_one();
		work.await.unwrap();
		let QueryResult::Utility(job) = core
			.query(&actor(), Query::Utility { job_id: id })
			.await
			.unwrap()
		else {
			panic!("Utility")
		};
		assert_eq!(
			(job.outcome, *peer.calls.lock().unwrap()),
			(
				UtilityOutcome::Refused {
					reason: "utility.policy_changed".into()
				},
				0
			)
		);
	}
	#[tokio::test]
	async fn restart_does_not_repeat_an_inference_whose_result_was_not_committed()
	 {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let peer = Arc::new(PausedInference::new(PauseAt::Inference));
		let core = start_core(&path).await.with_utility_host(peer.clone());
		let binding = configured(&core).await;
		let id = queue_rule(&core).await;
		{
			let work = core.perform_utilities();
			tokio::pin!(work);
			tokio::select! { () = peer.entered.notified() => {}, _ = &mut work => panic!("inference must pause") }
			core.perform_utilities().await.unwrap(); // Concurrent sweeps do not claim the same Effect.
		}
		core.close().await;
		let core = start_core(&path).await.with_utility_host(peer.clone());
		core.perform_utilities().await.unwrap();
		core.perform_utilities().await.unwrap();
		let QueryResult::Utility(job) = core
			.query(&actor(), Query::Utility { job_id: id })
			.await
			.unwrap()
		else {
			panic!("Utility")
		};
		assert_eq!(
			(
				job.binding_id,
				job.model,
				job.outcome,
				*peer.calls.lock().unwrap()
			),
			(
				Some(binding.binding_id),
				Some("small-model-v1".into()),
				UtilityOutcome::Refused {
					reason: "utility.interrupted".into()
				},
				1
			)
		);
	}
}
