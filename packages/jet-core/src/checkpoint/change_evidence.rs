//! Only durable activity receipts matching a complete content chain imply origin.
use crate::{
	ChangeEvidence, ChangeOrigin, ChangedFile, Core, CoreError, RunId,
	run::state::{self as run_state, State},
};
use jet_store::WriteTransaction;

#[derive(Clone, Copy)]
pub(crate) enum Window {
	ActiveTurn,
	BetweenTurns,
}

impl Core {
	/// Records a verified receipt from an in-process User-edit or terminal Adapter.
	/// The caller must derive identity from the authenticated Command or owned
	/// terminal and supply the content observed before and after that operation.
	/// This trusted integration entry point is never exposed to clients or Crafts;
	/// Craft evidence enters through Run observations and is always Harness-owned.
	///
	/// # Errors
	/// Rejects malformed or out-of-turn evidence. Conflicts and receipt overflow
	/// durably mark attribution incomplete without preventing turn capture.
	pub async fn record_change_evidence(
		&self,
		run_id: RunId,
		evidence: ChangeEvidence,
	) -> Result<(), CoreError> {
		self.store
			.write(async |tx| record(self, tx, run_id, evidence).await)
			.await
	}
}
pub(crate) async fn record(
	core: &Core,
	tx: &mut WriteTransaction,
	run_id: RunId,
	evidence: ChangeEvidence,
) -> Result<(), CoreError> {
	record_in(core, tx, run_id, evidence, Window::ActiveTurn).await
}
pub(crate) async fn record_between_turns(
	core: &Core,
	tx: &mut WriteTransaction,
	run_id: RunId,
	evidence: ChangeEvidence,
) -> Result<(), CoreError> {
	record_in(core, tx, run_id, evidence, Window::BetweenTurns).await
}
async fn record_in(
	core: &Core,
	tx: &mut WriteTransaction,
	run_id: RunId,
	evidence: ChangeEvidence,
	window: Window,
) -> Result<(), CoreError> {
	validate(run_id, &evidence)?;
	let execution = tx.run_execution(run_id.0).await?.ok_or_else(invalid)?;
	let mut state: State = run_state::decode(&execution.state)?;
	let run = tx.run(run_id.0).await?.ok_or_else(invalid)?;
	let tracking = state.changes.as_mut().ok_or_else(invalid)?;
	let turn = tracking.completed + 1;
	let (receipts, incomplete) = match window {
		Window::ActiveTurn if tracking.active.is_some() => {
			(&mut tracking.evidence, &mut tracking.evidence_incomplete)
		}
		Window::BetweenTurns
			if tracking.active.is_none() && !run.lifecycle.is_terminal() =>
		{
			(
				&mut tracking.between_turn_evidence,
				&mut tracking.between_turn_evidence_incomplete,
			)
		}
		Window::ActiveTurn | Window::BetweenTurns => return Err(invalid()),
	};
	if *incomplete {
		return Ok(());
	}
	let prior = receipts.iter().find(|prior| {
		prior.activity_id == evidence.activity_id && prior.path == evidence.path
	});
	if prior == Some(&evidence) {
		return Ok(());
	}
	if prior.is_some() || receipts.len() >= 256 {
		*incomplete = true;
	} else {
		receipts.push(evidence.clone());
	}
	let plan: crate::LaunchPlan = run_state::decode(&execution.plan)?;
	tx.append_event(
		crate::EventKind::ChangeEvidenceRecorded { turn, evidence }
			.to_record_as(
				crate::EventActor::RunSupervisor {
					run_id,
					authorized_by: plan.client_id,
				},
				crate::event::EventSubject::Run {
					conversation_id: crate::ConversationId(run.conversation_id),
					run_id,
				},
				core.now_unix_ms(),
			)?,
	)
	.await?;
	tx.update_run_execution(
		run_id.0,
		&serde_json::to_string(&state)
			.map_err(crate::checkpoint::change_artifact::failed)?,
	)
	.await?;
	Ok(())
}
fn validate(run_id: RunId, evidence: &ChangeEvidence) -> Result<(), CoreError> {
	// ASVS 2.2.1/2.2.2: bounded exact content identities at the trusted seam.
	crate::RelativePath::parse(&evidence.path)?;
	let source_valid = match evidence.origin {
		ChangeOrigin::Harness { run_id: source } => source == run_id,
		ChangeOrigin::UserEdit { .. }
		| ChangeOrigin::WorkspaceTerminal { .. } => true,
		ChangeOrigin::Mixed | ChangeOrigin::ExternalOrUnknown => false,
	};
	let object_valid = |value: &str| {
		matches!(value.len(), 40 | 64)
			&& value
				.bytes()
				.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	};
	let mode_valid = |value: &str| {
		matches!(value, "000000" | "100644" | "100755" | "120000" | "160000")
	};
	if !source_valid
		|| evidence.activity_id.is_empty()
		|| evidence.activity_id.len() > 256
		|| !object_valid(&evidence.before_object)
		|| !object_valid(&evidence.after_object)
		|| !mode_valid(&evidence.before_mode)
		|| !mode_valid(&evidence.after_mode)
	{
		return Err(invalid());
	}
	Ok(())
}
pub(crate) fn attribute(
	files: &mut [ChangedFile],
	evidence: &[ChangeEvidence],
) {
	for file in files {
		let (Some(mut object), Some(after_object)) =
			(file.before_object.as_deref(), file.after_object.as_deref())
		else {
			file.origin = ChangeOrigin::ExternalOrUnknown;
			continue;
		};
		let mut mode = file.before_mode.as_str();
		let mut origin = None;
		let mut complete = true;
		for receipt in evidence.iter().filter(|e| e.path == file.path) {
			if receipt.before_object != object || receipt.before_mode != mode {
				complete = false;
				break;
			}
			object = &receipt.after_object;
			mode = &receipt.after_mode;
			origin = Some(match origin {
				None => receipt.origin.clone(),
				Some(ChangeOrigin::ExternalOrUnknown) => {
					ChangeOrigin::ExternalOrUnknown
				}
				Some(_)
					if receipt.origin == ChangeOrigin::ExternalOrUnknown =>
				{
					ChangeOrigin::ExternalOrUnknown
				}
				Some(prior) if prior == receipt.origin => prior,
				Some(_) => ChangeOrigin::Mixed,
			});
		}
		file.origin =
			if complete && object == after_object && mode == file.after_mode {
				origin.unwrap_or(ChangeOrigin::ExternalOrUnknown)
			} else {
				ChangeOrigin::ExternalOrUnknown
			};
	}
}
fn invalid() -> CoreError {
	CoreError::invalid_input(
		"checkpoint.invalid_evidence",
		"change evidence must identify a bounded, verified operation in a tracked change window",
	)
}
