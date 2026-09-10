//! Diff selection reads durable boundaries at one Event fence.
use crate::{
	ChangeCheckpoint, ChangeDiff, Core, CoreError, DiffScope, EventSequence,
	PlaneId, QueryResult, RunId, WorkspaceId,
};
use crate::{
	change_artifact, checkpoint_capture,
	checkpoint_state::missing,
	run_state::{self, State},
};

pub(crate) async fn query(
	core: &Core,
	run_id: RunId,
	scope: DiffScope,
	start: crate::checkpoint_pages::Start,
) -> Result<QueryResult, CoreError> {
	let limits = core.artifact_policy().await?;
	let (mut diff, root, checkpoints, tracking) = core
		.store
		.read(async |tx| {
			let run = tx.run(run_id.0).await?.ok_or_else(missing)?;
			let execution =
				tx.run_execution(run_id.0).await?.ok_or_else(missing)?;
			let state: State = run_state::decode(&execution.state)?;
			let tracking = state.changes.ok_or_else(missing)?;
			let plan: crate::LaunchPlan = run_state::decode(&execution.plan)?;
			let (from, to) = match scope {
				DiffScope::Turn { turn } => (turn, turn),
				DiffScope::Current => (0, 0),
				DiffScope::Final if run.lifecycle.is_terminal() => {
					(0, tracking.completed)
				}
				DiffScope::Final => {
					return Err(CoreError::conflict(
						"checkpoint.run_active",
						"the Run has no final diff yet",
					));
				}
				DiffScope::Historical { from_turn, to_turn }
					if from_turn <= to_turn =>
				{
					(from_turn, to_turn)
				}
				DiffScope::Historical { .. } => {
					return Err(CoreError::invalid_input(
						"checkpoint.invalid_range",
						"historical boundaries must be in ascending order",
					));
				}
			};
			let before = if from == 0 {
				tracking.baseline.clone()
			} else {
				let c: ChangeCheckpoint = run_state::decode(
					&tx.change_checkpoint(run_id.0, from)
						.await?
						.ok_or_else(missing)?,
				)?;
				if matches!(scope, DiffScope::Turn { .. }) {
					c.before
				} else {
					c.after
				}
			};
			let (after, outcome, files, artifact) = if scope == DiffScope::Final
			{
				let after = tracking.terminal.clone().ok_or_else(missing)?;
				let artifact = after.uncommitted.clone();
				(after, None, vec![], artifact)
			} else if to == 0 && !matches!(scope, DiffScope::Turn { .. }) {
				(
					tracking.baseline.clone(),
					None,
					vec![],
					tracking.baseline.uncommitted.clone(),
				)
			} else {
				let c: ChangeCheckpoint = run_state::decode(
					&tx.change_checkpoint(run_id.0, to)
						.await?
						.ok_or_else(missing)?,
				)?;
				(
					c.after,
					matches!(scope, DiffScope::Turn { .. })
						.then_some(c.outcome),
					c.files,
					c.artifact,
				)
			};
			let mut checkpoints = Vec::new();
			if !matches!(scope, DiffScope::Turn { .. }) {
				let end = if scope == DiffScope::Current {
					tracking.completed
				} else {
					to
				};
				for turn in (from + 1)..=end {
					let c: ChangeCheckpoint = run_state::decode(
						&tx.change_checkpoint(run_id.0, turn)
							.await?
							.ok_or_else(missing)?,
					)?;
					checkpoints.push(c);
				}
			}
			Ok::<_, CoreError>((
				ChangeDiff {
					total_files: 0,
					next_page: None,
					cursor: EventSequence(tx.event_cursor().await?),
					plane_id: PlaneId(tx.plane().await?.plane_id),
					workspace_id: tx
						.workspace_of(run.conversation_id)
						.await?
						.map(|w| WorkspaceId(w.workspace_id)),
					run_id,
					scope: scope.clone(),
					latest_turn: tracking.completed,
					before,
					after,
					outcome,
					files,
					artifact,
					patch: String::new(),
					patch_truncated: false,
				},
				plan.root,
				checkpoints,
				tracking,
			))
		})
		.await?;
	if scope == DiffScope::Current {
		diff.after = checkpoint_capture::snapshot(
			core,
			&root,
			run_id,
			checkpoint_capture::Retention::Current,
			limits,
		)
		.await?;
	}
	if !matches!(scope, DiffScope::Turn { .. }) {
		diff.files =
			checkpoint_capture::files(&root, &diff.before, &diff.after).await?;
		let mut evidence = Vec::new();
		let mut previous = diff.before.clone();
		for checkpoint in checkpoints {
			let mut before_files =
				checkpoint_capture::files(&root, &previous, &checkpoint.before)
					.await?;
			if !checkpoint.before_evidence_incomplete {
				crate::change_evidence::attribute(
					&mut before_files,
					&checkpoint.before_evidence,
				);
			}
			append_transitions(&mut evidence, before_files);
			append_transitions(&mut evidence, checkpoint.files);
			previous = checkpoint.after;
		}
		if scope == DiffScope::Current {
			if let Some(active) = tracking.active {
				let mut before_files =
					checkpoint_capture::files(&root, &previous, &active)
						.await?;
				if !tracking.between_turn_evidence_incomplete {
					crate::change_evidence::attribute(
						&mut before_files,
						&tracking.between_turn_evidence,
					);
				}
				append_transitions(&mut evidence, before_files);
				let mut active_files =
					checkpoint_capture::files(&root, &active, &diff.after)
						.await?;
				if !tracking.evidence_incomplete {
					crate::change_evidence::attribute(
						&mut active_files,
						&tracking.evidence,
					);
				}
				append_transitions(&mut evidence, active_files);
			} else {
				let mut between_files =
					checkpoint_capture::files(&root, &previous, &diff.after)
						.await?;
				if !tracking.between_turn_evidence_incomplete {
					crate::change_evidence::attribute(
						&mut between_files,
						&tracking.between_turn_evidence,
					);
				}
				append_transitions(&mut evidence, between_files);
			}
		} else if scope == DiffScope::Final {
			let mut terminal_files =
				checkpoint_capture::files(&root, &previous, &diff.after)
					.await?;
			if !tracking.between_turn_evidence_incomplete {
				crate::change_evidence::attribute(
					&mut terminal_files,
					&tracking.between_turn_evidence,
				);
			}
			append_transitions(&mut evidence, terminal_files);
		}
		crate::change_evidence::attribute(&mut diff.files, &evidence);
		diff.artifact = if crate::checkpoint_pressure::incomplete(&diff.before)
			|| crate::checkpoint_pressure::incomplete(&diff.after)
		{
			crate::checkpoint_pressure::artifact()
		} else {
			checkpoint_capture::patch(
				core,
				&root,
				run_id,
				&diff.before.tree,
				&diff.after.tree,
				limits,
				checkpoint_capture::Retention::Current,
			)
			.await?
		};
	}
	if matches!(scope, DiffScope::Historical { from_turn, to_turn } if from_turn == to_turn)
	{
		diff.files.clear();
	}
	crate::checkpoint_pages::bound_page(core, &mut diff, start)?;
	diff.patch =
		change_artifact::preview(core.run_home(), &diff.artifact).await?;
	diff.patch_truncated = diff.artifact.availability
		!= crate::ArtifactAvailability::Stored
		|| diff.artifact.size > change_artifact::PREVIEW_LIMIT as u64;
	Ok(QueryResult::ChangeDiff(Box::new(diff)))
}

// Compose verified checkpoint transitions, including unknown steps and idle
// gaps. Raw receipts cannot upgrade a boundary already known to be unexplained.
fn append_transitions(
	evidence: &mut Vec<crate::ChangeEvidence>,
	files: Vec<crate::ChangedFile>,
) {
	evidence.extend(files.into_iter().map(|file| crate::ChangeEvidence {
		activity_id: String::new(),
		origin: file.origin,
		path: file.path,
		before_object: file.before_object.unwrap_or_default(),
		after_object: file.after_object.unwrap_or_default(),
		before_mode: file.before_mode,
		after_mode: file.after_mode,
	}));
}

pub(crate) async fn next(
	core: &Core,
	cursor: crate::PageCursor,
) -> Result<QueryResult, CoreError> {
	let Some(page) = core.checkpoint_pages.resume(cursor, core.now_unix_ms())
	else {
		let current = core
			.store
			.read(async |tx| {
				Ok::<_, CoreError>(EventSequence(tx.event_cursor().await?))
			})
			.await?;
		return Err(CoreError::pagination_stale(current));
	};
	query(
		core,
		page.run_id,
		page.scope.clone(),
		crate::checkpoint_pages::Start::After(Box::new(page)),
	)
	.await
}
