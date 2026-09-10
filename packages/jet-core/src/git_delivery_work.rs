//! Committed Git Effects are attempted once and reconciled after interruption.
use crate::git_delivery_io as io;
use crate::git_delivery_state::{self as state, Document, refused};
use crate::{
	Core, CoreError, GitDeliveryOutcome, GitOperation, UtilityOutcome,
};
use jet_store::{EffectKindRecord, EffectStateRecord, WriteTransaction};

impl Core {
	/// Settle committed delivery operations and reconcile interrupted mutations.
	/// # Errors
	/// Returns persistence failures without claiming an unrecorded outcome.
	#[expect(
		clippy::await_holding_invalid_type,
		reason = "serialize Git publication with other Effect workers"
	)]
	pub async fn perform_git_deliveries(&self) -> Result<(), CoreError> {
		self.perform_utilities().await?;
		let _guard = self.effect_reconciliation.lock().await;
		let effects = self
			.store
			.read(async |tx| {
				tx.unresolved_effects_of(EffectKindRecord::GitDelivery)
					.await
			})
			.await?;
		for effect in effects {
			let mut doc = self
				.store
				.read(async |tx| state::load(tx, effect.effect_id).await)
				.await?;
			if effect.state != EffectStateRecord::Pending {
				let outcome = self.reconcile_git(&doc).await;
				self.store
					.write(async |tx| settle(tx, &mut doc, outcome).await)
					.await?;
				continue;
			}
			let prepared = self.prepare_git(&mut doc).await;
			if let Err(error) = prepared {
				if matches!(
					error.code.as_str(),
					"git.utility_pending"
						| "git.run_active" | "git.previous_step_pending"
				) {
					continue;
				}
				self.store
					.write(async |tx| {
						tx.begin_effect_attempt(effect.effect_id).await?;
						settle(
							tx,
							&mut doc,
							GitDeliveryOutcome::Failed { code: error.code },
						)
						.await
					})
					.await?;
				continue;
			}
			// Persist the exact commit object and attempt before changing refs or calling GitHub.
			self.store
				.write(async |tx| {
					state::save(tx, &doc).await?;
					tx.begin_effect_attempt(effect.effect_id).await?;
					Ok::<_, CoreError>(())
				})
				.await?;
			let outcome = self
				.store
				.write(async |tx| {
					// The transaction serializes final policy checks and native-turn admission
					// with this previously committed Effect (ASVS 2.3.3, 2.3.4).
					if let Err(error) = self.validate_git(tx, &doc).await {
						return settle(
							tx,
							&mut doc,
							GitDeliveryOutcome::Failed { code: error.code },
						)
						.await;
					}
					let result = match &doc.delivery.operation {
						GitOperation::Branch { name } => {
							if doc.delivery.policy.automatic
								&& doc.branch.as_ref() == Some(name)
							{
								Ok(None)
							} else {
								io::git(
									&doc.root,
									&["switch", "--no-guess", "-c", name],
								)
								.await
								.map(|_| None)
							}
						}
						GitOperation::Commit => {
							io::commit(&doc, &self.workspace_home)
								.await
								.map(|_| None)
						}
						GitOperation::Push { .. } => {
							io::push(&doc, io::PushMode::Execute)
								.await
								.map(|_| None)
						}
						GitOperation::DraftPullRequest { .. } => {
							crate::git_delivery_github::apply(
								self.github_host.as_ref(),
								&doc,
							)
							.await
							.map(Some)
						}
					};
					let outcome = match result {
						Ok(url) => completed(&doc, url).await,
						// Even a native nonzero exit can follow a successful remote write.
						Err(_) => self.reconcile_git(&doc).await,
					};
					settle(tx, &mut doc, outcome).await
				})
				.await;
			outcome?;
		}
		self.turn_wake.send_replace(());
		self.run_work.notify_one();
		Ok(())
	}
	async fn prepare_git(&self, doc: &mut Document) -> Result<(), CoreError> {
		self.security()
			.await
			.admit(crate::security::SecurityClass::Guarded)?;
		if !self.observe_capabilities().await.supports(
			crate::capability::Capability::ExternalTool(
				crate::ExternalTool::Git,
			),
		) {
			return Err(refused("git.tool_unavailable"));
		}
		self.store
			.read(async |tx| {
				if let Some(previous) = doc.previous {
					let previous = state::load(tx, previous).await?;
					if previous.delivery.outcome == GitDeliveryOutcome::Pending
					{
						return Err(refused("git.previous_step_pending"));
					}
					let GitDeliveryOutcome::Completed { head, branch, .. } =
						previous.delivery.outcome
					else {
						return Err(refused("git.previous_step_incomplete"));
					};
					doc.head = head;
					doc.branch = branch;
					if matches!(
						previous.delivery.operation,
						GitOperation::Commit
					) && previous.prepared_commit.as_ref()
						!= Some(&previous.head)
					{
						doc.index = previous.tree.ok_or_else(|| {
							refused("git.checkpoint_required")
						})?;
					} else {
						doc.index = previous.index;
					}
				}
				if let Some(id) = doc.delivery.utility_job {
					match crate::utility_work::query(tx, id).await?.outcome {
						UtilityOutcome::Text {
							text,
							body,
							fallback_reason,
						} => {
							doc.delivery.message = Some(crate::GitMessage {
								title: text,
								body,
								fallback_reason,
							});
						}
						UtilityOutcome::Pending => {
							return Err(refused("git.utility_pending"));
						}
						UtilityOutcome::Draft { .. }
						| UtilityOutcome::Refused { .. } => {
							return Err(refused("git.utility_invalid"));
						}
					}
				}
				Ok::<_, CoreError>(())
			})
			.await?;
		self.store
			.read(async |tx| self.validate_git(tx, doc).await)
			.await?;
		match &doc.delivery.operation {
			GitOperation::Commit => {
				doc.prepared_commit = Some(
					io::prepare_commit(doc, doc.title(), doc.body()).await?,
				)
			}
			GitOperation::Push { .. } => {
				io::push(doc, io::PushMode::Preflight).await?
			}
			GitOperation::DraftPullRequest { .. } => {
				if !self
					.observe_capabilities()
					.await
					.supports(crate::capability::Capability::CredentialStore)
				{
					return Err(refused("git.github_credential_unavailable"));
				}
				crate::git_delivery_github::preflight(
					self.github_host.as_ref(),
					doc,
				)
				.await?;
			}
			GitOperation::Branch { name } => {
				io::git(&doc.root, &["check-ref-format", "--branch", name])
					.await?;
				if !doc.delivery.policy.automatic
					|| doc.branch.as_ref() != Some(name)
				{
					let branches = io::git(
						&doc.root,
						&[
							"for-each-ref",
							"--format=%(refname)",
							&format!("refs/heads/{name}"),
						],
					)
					.await?;
					if !branches.is_empty() {
						return Err(refused("git.branch_exists"));
					}
				}
			}
		}
		Ok(())
	}
	async fn validate_git(
		&self,
		tx: &mut jet_store::ReadTransaction,
		doc: &Document,
	) -> Result<(), CoreError> {
		state::idle(tx, doc.delivery.conversation_id).await?;
		if state::root(tx, doc.delivery.conversation_id).await? != doc.root {
			return Err(refused("git.root_changed"));
		}
		let mut current =
			state::policy(tx, doc.delivery.conversation_id).await?;
		current.automatic = doc.delivery.policy.automatic;
		if current != doc.delivery.policy {
			return Err(refused("git.policy_changed"));
		}
		let observed = io::inspect(&doc.root, &doc.delivery.operation).await?;
		if (
			observed.head,
			observed.index,
			observed.branch,
			observed.remote_url,
		) != (
			doc.head.clone(),
			doc.index.clone(),
			doc.branch.clone(),
			doc.remote_url.clone(),
		) {
			return Err(refused("git.repository_changed"));
		}
		if let Some(tree) = &doc.tree {
			let current = crate::workspace::with_scratch(
				&self.workspace_home,
				"delivery-check",
				async |scratch| {
					let path = scratch.join("index");
					let index = crate::tree_capture::ScratchIndex::new(
						&doc.root,
						&path,
						|_| refused("git.inspect_failed"),
					);
					index.copy_from_checkout().await?;
					Ok(index.capture_everything(&doc.head).await?.0)
				},
			)
			.await?;
			if &current != tree {
				return Err(refused("git.content_changed"));
			}
		}
		Ok(())
	}
	async fn reconcile_git(&self, doc: &Document) -> GitDeliveryOutcome {
		let observed = io::inspect(&doc.root, &doc.delivery.operation).await;
		let Ok(observed) = observed else {
			return GitDeliveryOutcome::OutcomeUnknown;
		};
		if observed.remote_url != doc.remote_url {
			return GitDeliveryOutcome::OutcomeUnknown;
		}
		let done = match &doc.delivery.operation {
			GitOperation::Branch { name } => {
				observed.head == doc.head
					&& observed.branch.as_deref() == Some(name)
			}
			GitOperation::Commit => {
				doc.prepared_commit.as_ref() == Some(&observed.head)
					&& doc.tree.as_ref() == Some(&observed.index)
			}
			GitOperation::Push { .. } => io::pushed(doc).await.unwrap_or(false),
			GitOperation::DraftPullRequest { .. } => {
				return match crate::git_delivery_github::reconcile(
					self.github_host.as_ref(),
					doc,
				)
				.await
				{
					Ok(Some(url)) => completed(doc, Some(url)).await,
					Ok(None) | Err(_) => GitDeliveryOutcome::OutcomeUnknown,
				};
			}
		};
		if done {
			completed(doc, None).await
		} else {
			GitDeliveryOutcome::OutcomeUnknown
		}
	}
}
async fn completed(
	doc: &Document,
	pull_request: Option<String>,
) -> GitDeliveryOutcome {
	match (
		crate::worktree::resolve_commit(&doc.root, "HEAD").await,
		io::branch(&doc.root).await,
	) {
		(Ok(head), Ok(branch)) => GitDeliveryOutcome::Completed {
			head,
			branch,
			pull_request,
		},
		_ => GitDeliveryOutcome::OutcomeUnknown,
	}
}
async fn settle(
	tx: &mut WriteTransaction,
	doc: &mut Document,
	outcome: GitDeliveryOutcome,
) -> Result<(), CoreError> {
	let state = match outcome {
		GitDeliveryOutcome::Completed { .. } => EffectStateRecord::Completed,
		GitDeliveryOutcome::Failed { .. } => EffectStateRecord::Failed,
		GitDeliveryOutcome::OutcomeUnknown => EffectStateRecord::OutcomeUnknown,
		GitDeliveryOutcome::Pending => {
			return Err(refused("git.invalid_outcome"));
		}
	};
	doc.delivery.outcome = outcome;
	if let GitDeliveryOutcome::Completed {
		pull_request: Some(url),
		..
	} = &doc.delivery.outcome
	{
		tx.save_git_draft(doc.delivery.conversation_id.0, url)
			.await?;
	}
	if let (
		GitOperation::Branch { name },
		GitDeliveryOutcome::Completed {
			branch: Some(branch),
			..
		},
	) = (&doc.delivery.operation, &doc.delivery.outcome)
		&& branch == name
	{
		tx.save_git_branch(doc.delivery.conversation_id.0, name)
			.await?;
	}
	state::save(tx, doc).await?;
	tx.finish_effect(doc.delivery.delivery_id, state).await?;
	Ok(())
}
