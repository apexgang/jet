//! Resolve allowlisted inputs from authoritative Run and checkpoint records.
use crate::{
	Core, CoreError, UtilityInput, UtilityOutcome, UtilityRequest,
	utility::work::unavailable,
};

impl Core {
	pub(crate) async fn utility_input(
		&self,
		request: &UtilityRequest,
	) -> Result<UtilityInput, CoreError> {
		match request {
			UtilityRequest::Autodelete { prompt } => {
				Ok(UtilityInput::Autodelete {
					prompt: prompt.clone(),
				})
			}
			UtilityRequest::GitText { run_id, turn } => {
				let (checkpoint, instructions) = self
					.store
					.read(async |tx| {
						let json = tx
							.change_checkpoint(run_id.0, *turn)
							.await?
							.ok_or_else(|| {
								unavailable("utility.checkpoint_unavailable")
							})?;
						let checkpoint: crate::ChangeCheckpoint =
							crate::run::state::decode(&json)?;
						let instructions = crate::setting::resolve_plane(
							tx,
							crate::SettingKey::GitMessageInstructions,
						)
						.await?;
						let crate::SettingValue::Text(instructions) =
							instructions
						else {
							return Err(unavailable(
								"utility.instructions_invalid",
							));
						};
						Ok::<_, CoreError>((checkpoint, instructions))
					})
					.await?;
				if checkpoint.artifact.availability
					!= crate::ArtifactAvailability::Stored
				{
					return Err(unavailable("utility.checkpoint_unavailable"));
				}
				let patch = crate::checkpoint::change_artifact::preview(
					self.run_home(),
					&checkpoint.artifact,
				)
				.await?;
				Ok(UtilityInput::GitText {
					patch: bounded(&patch, 16384),
					instructions,
				})
			}
			UtilityRequest::Naming { run_id } => {
				self.store
					.read(async |tx| {
						let run = tx.run(run_id.0).await?.ok_or_else(|| {
							unavailable("utility.source_unavailable")
						})?;
						let opening_context = match tx
							.run_execution(run_id.0)
							.await?
						{
							Some(execution) => {
								let plan: crate::LaunchPlan =
									crate::run::state::decode(&execution.plan)?;
								bounded(&plan.prompt, 4096)
							}
							None => String::new(),
						};
						Ok(UtilityInput::Naming {
							title: bounded(
								&crate::Run::from(run).name.value,
								256,
							),
							opening_context,
						})
					})
					.await
			}
		}
	}
	pub(crate) async fn utility_fallback(
		&self,
		request: &UtilityRequest,
		reason: String,
	) -> UtilityOutcome {
		match request {
			UtilityRequest::Autodelete { .. } => {
				UtilityOutcome::Refused { reason }
			}
			UtilityRequest::GitText { turn, .. } => UtilityOutcome::Text {
				text: format!("Update Run changes (turn {turn})"),
				body: String::new(),
				fallback_reason: Some(reason),
			},
			UtilityRequest::Naming { run_id } => {
				let text = self
					.store
					.read(async |tx| tx.run(run_id.0).await)
					.await
					.ok()
					.flatten()
					.map_or_else(
						|| format!("Run {}", run_id.0),
						|run| crate::Run::from(run).name.value,
					);
				UtilityOutcome::Text {
					text,
					body: String::new(),
					fallback_reason: Some(reason),
				}
			}
		}
	}
}
fn bounded(text: &str, limit: usize) -> String {
	let mut end = text.len().min(limit);
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	text[..end].into()
}
