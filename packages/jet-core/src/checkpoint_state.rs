//! Checkpoints commit with the source receipt that completed their turn.
use crate::{
	ChangeCheckpoint, ChangeSnapshot, ConversationId, Core, CoreError, PlaneId,
	RunId, TurnOutcome, WorkspaceId,
};
use crate::{
	change_artifact, checkpoint_capture,
	run_state::{self, Observation, State},
};
use jet_store::WriteTransaction;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Tracking {
	#[serde(default)]
	pub(crate) between_turn_evidence: Vec<crate::ChangeEvidence>,
	#[serde(default)]
	pub(crate) between_turn_evidence_incomplete: bool,
	#[serde(default)]
	pub(crate) evidence: Vec<crate::ChangeEvidence>,
	#[serde(default)]
	pub(crate) evidence_incomplete: bool,
	pub(crate) baseline: ChangeSnapshot,
	pub(crate) active: Option<ChangeSnapshot>,
	#[serde(default)]
	pub(crate) terminal: Option<ChangeSnapshot>,
	pub(crate) completed: u32,
}
impl Core {
	pub(crate) async fn begin_run_changes(
		&self,
		run_id: RunId,
		plan: &crate::LaunchPlan,
	) -> Result<(), CoreError> {
		let limits = self.artifact_policy().await?;
		let before = checkpoint_capture::snapshot(
			self,
			&plan.root,
			run_id,
			checkpoint_capture::Retention::Durable,
			limits,
		)
		.await?;
		self.store
			.write(async |tx| {
				let execution =
					tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
				let mut state: State = run_state::decode(&execution.state)?;
				if state.changes.is_some() {
					return Err(change_artifact::failed(
						"Run capture already exists",
					));
				}
				state.changes = Some(Tracking {
					between_turn_evidence: vec![],
					between_turn_evidence_incomplete: false,
					evidence: vec![],
					evidence_incomplete: false,
					baseline: before.clone(),
					active: Some(before),
					terminal: None,
					completed: 0,
				});
				save(tx, run_id, &state).await
			})
			.await
	}
}
pub(crate) async fn observe(
	core: &Core,
	tx: &mut WriteTransaction,
	run_id: RunId,
	observation: &Observation,
) -> Result<(), CoreError> {
	let limits = core.artifact_policy_in(tx).await?;
	if let Observation::FileChanged(evidence) = observation {
		if evidence.origin != (crate::ChangeOrigin::Harness { run_id }) {
			return Err(change_artifact::failed(
				"a Craft cannot claim another origin",
			));
		}
		return crate::change_evidence::record(
			core,
			tx,
			run_id,
			evidence.clone(),
		)
		.await;
	}
	if matches!(observation, Observation::TurnStarted) {
		let execution =
			tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
		let mut state: State = run_state::decode(&execution.state)?;
		let tracking = state.changes.as_mut().ok_or_else(missing)?;
		if tracking.active.is_some() {
			// The queue captures before delivery; a Craft may report that same
			// start afterward. Keep the earlier durable boundary.
			return Ok(());
		}
		let plan: crate::LaunchPlan = run_state::decode(&execution.plan)?;
		tracking.active = Some(
			checkpoint_capture::snapshot(
				core,
				&plan.root,
				run_id,
				checkpoint_capture::Retention::Durable,
				limits,
			)
			.await?,
		);
		return save(tx, run_id, &state).await;
	}
	let terminal = matches!(
		observation,
		Observation::Ended(_) | Observation::Lost | Observation::LaunchFailed
	);
	let outcome = match observation {
		Observation::Completed(_)
		| Observation::NativeConversation(_)
		| Observation::TurnCompleted { .. } => TurnOutcome::Completed,
		Observation::TurnEnded(outcome) => *outcome,
		Observation::Ended(_)
		| Observation::Lost
		| Observation::LaunchFailed => TurnOutcome::Interrupted,
		_ => return Ok(()),
	};
	let execution = tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
	let mut state: State = run_state::decode(&execution.state)?;
	let Some(tracking) = &mut state.changes else {
		return Ok(());
	};
	let plan: crate::LaunchPlan = run_state::decode(&execution.plan)?;
	let Some(before) = tracking.active.take() else {
		if terminal {
			tracking.terminal = Some(
				checkpoint_capture::snapshot(
					core,
					&plan.root,
					run_id,
					checkpoint_capture::Retention::Durable,
					limits,
				)
				.await?,
			);
			return save(tx, run_id, &state).await;
		}
		return Ok(());
	};
	let after = checkpoint_capture::snapshot(
		core,
		&plan.root,
		run_id,
		checkpoint_capture::Retention::Durable,
		limits,
	)
	.await?;
	let mut files =
		checkpoint_capture::files(&plan.root, &before, &after).await?;
	if terminal {
		tracking.terminal = Some(after.clone());
	}
	if !tracking.evidence_incomplete {
		crate::change_evidence::attribute(&mut files, &tracking.evidence);
	}
	let artifact = if crate::checkpoint_pressure::incomplete(&before)
		|| crate::checkpoint_pressure::incomplete(&after)
	{
		crate::checkpoint_pressure::artifact()
	} else {
		checkpoint_capture::patch(
			core,
			&plan.root,
			run_id,
			&before.tree,
			&after.tree,
			limits,
			checkpoint_capture::Retention::Durable,
		)
		.await?
	};
	let run = tx.run(run_id.0).await?.ok_or_else(missing)?;
	tracking.completed += 1;
	let checkpoint = ChangeCheckpoint {
		before_evidence: std::mem::take(&mut tracking.between_turn_evidence),
		before_evidence_incomplete: std::mem::take(
			&mut tracking.between_turn_evidence_incomplete,
		),
		evidence: std::mem::take(&mut tracking.evidence),
		evidence_incomplete: std::mem::take(&mut tracking.evidence_incomplete),
		plane_id: PlaneId(tx.plane().await?.plane_id),
		workspace_id: tx
			.workspace_of(run.conversation_id)
			.await?
			.map(|w| WorkspaceId(w.workspace_id)),
		conversation_id: ConversationId(run.conversation_id),
		run_id,
		turn: tracking.completed,
		outcome,
		before,
		after,
		files,
		artifact,
	};
	// ASVS 2.3.3: the immutable record, semantic Event, replay prefix, and
	// active-turn projection are one transaction; the Artifact was synced first.
	tx.insert_change_checkpoint(
		run_id.0,
		checkpoint.turn,
		&serde_json::to_string(&checkpoint).map_err(change_artifact::failed)?,
	)
	.await?;
	tx.append_event(
		crate::EventKind::ChangeCheckpointRecorded {
			turn: checkpoint.turn,
			outcome,
			artifact: checkpoint.artifact.clone(),
		}
		.to_record_as(
			crate::EventActor::RunSupervisor {
				run_id,
				authorized_by: plan.client_id,
			},
			crate::event::EventSubject::Run {
				conversation_id: checkpoint.conversation_id,
				run_id,
			},
			core.now_unix_ms(),
		)?,
	)
	.await?;
	crate::git_delivery_state::automate(tx, &checkpoint, plan.client_id)
		.await?;
	core.utility_wake.notify_one();
	save(tx, run_id, &state).await
}
async fn save(
	tx: &mut WriteTransaction,
	run_id: RunId,
	state: &State,
) -> Result<(), CoreError> {
	tx.update_run_execution(
		run_id.0,
		&serde_json::to_string(state).map_err(change_artifact::failed)?,
	)
	.await?;
	Ok(())
}
pub(crate) fn missing() -> CoreError {
	CoreError::not_found(
		"checkpoint.not_found",
		"the requested Change checkpoint does not exist",
	)
}
