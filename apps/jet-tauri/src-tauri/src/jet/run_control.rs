use std::{collections::HashMap, sync::Mutex};

use jet_protocol::{
    RunActivity, RunControl, RunExecution, RunLifecycle, TerminationStage, Turn, TurnSource,
    TurnState,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use super::{
    conversations::{run_view, RunView},
    errors::PublicError,
    JetBridge,
};

const MAX_PENDING_COMMANDS: usize = 256;
const MAX_QUEUE_ENTRIES: usize = 128;
const MAX_PROMPT_BYTES: usize = 65_536;

#[derive(Default)]
pub(crate) struct RunControlState {
    withdraw: Mutex<HashMap<(Uuid, Uuid), Uuid>>,
    control: Mutex<HashMap<ControlKey, Uuid>>,
    approval_retry: Mutex<HashMap<(Uuid, Uuid), Uuid>>,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct ControlKey {
    run_id: Uuid,
    stop_run: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSupervisionView {
    cursor: String,
    maximum_entries: usize,
    maximum_prompt_bytes: usize,
    turns: Vec<TurnView>,
    execution: Option<RunExecutionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnView {
    id: String,
    sequence: String,
    position: usize,
    source: &'static str,
    state: &'static str,
    run_id: Option<String>,
    target: &'static str,
    withdrawable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunExecutionView {
    cursor: String,
    run: RunView,
    activity: Option<&'static str>,
    needs_attention: bool,
    termination: Option<TerminationView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminationView {
    control: &'static str,
    stage: &'static str,
    summary: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandAcceptedView {
    run: RunView,
    control: &'static str,
    message: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRetryView {
    review_id: String,
    message: &'static str,
}

pub(crate) async fn load_run_supervision(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    run_id: Option<String>,
) -> Result<RunSupervisionView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let run_id = run_id
        .map(|value| parse_id(&value, "run.identifier_invalid"))
        .transpose()?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let queue = client
        .turn_queue(conversation_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let execution = match run_id {
        Some(run_id) => {
            let execution = client
                .run_execution(run_id)
                .await
                .map_err(|error| PublicError::from_client(&error))?;
            if execution.run.conversation_id != conversation_id {
                return Err(PublicError::invalid_input(
                    "run.conversation_mismatch",
                    "That Run does not belong to the selected Conversation.",
                ));
            }
            Some(execution_view(execution))
        }
        None => None,
    };
    Ok(RunSupervisionView {
        cursor: queue.cursor.to_string(),
        maximum_entries: MAX_QUEUE_ENTRIES,
        maximum_prompt_bytes: MAX_PROMPT_BYTES,
        turns: queue
            .turns
            .into_iter()
            .enumerate()
            .map(|(index, turn)| turn_view(turn, index, bridge.client.client_id()))
            .collect(),
        execution,
    })
}

pub(crate) async fn withdraw_turn(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    turn_id: String,
) -> Result<TurnView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let turn_id = parse_id(&turn_id, "turn.identifier_invalid")?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let queue = client
        .turn_queue(conversation_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let Some(position) = queue.turns.iter().position(|turn| turn.turn_id == turn_id) else {
        return Err(PublicError::invalid_input(
            "turn.not_queued",
            "That Turn is no longer in this queue.",
        ));
    };
    let candidate = &queue.turns[position];
    // ASVS 2.2.3 and 8.3.1: narrow the untrusted webview request to the
    // authenticated client's own queued user input. The Plane rechecks it.
    if candidate.client_id != bridge.client.client_id()
        || candidate.source != TurnSource::User
        || candidate.state != TurnState::Queued
    {
        return Err(PublicError::invalid_input(
            "turn.withdraw_denied",
            "Only your own queued user Turn can be withdrawn.",
        ));
    }
    let key = (conversation_id, turn_id);
    let command_id = command_id(&bridge.run_control.withdraw, key)?;
    let turn = client
        .withdraw_turn(command_id, conversation_id, turn_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    remove_command(&bridge.run_control.withdraw, &key)?;
    Ok(turn_view(turn, position, bridge.client.client_id()))
}

pub(crate) async fn interrupt_turn(
    bridge: State<'_, JetBridge>,
    run_id: String,
) -> Result<CommandAcceptedView, PublicError> {
    control_run(bridge, run_id, RunControl::InterruptTurn).await
}

pub(crate) async fn stop_run(
    bridge: State<'_, JetBridge>,
    run_id: String,
) -> Result<CommandAcceptedView, PublicError> {
    control_run(bridge, run_id, RunControl::StopRun).await
}

async fn control_run(
    bridge: State<'_, JetBridge>,
    run_id: String,
    control: RunControl,
) -> Result<CommandAcceptedView, PublicError> {
    let run_id = parse_id(&run_id, "run.identifier_invalid")?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let execution = client
        .run_execution(run_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    if execution.run.lifecycle != RunLifecycle::Active {
        return Err(PublicError::invalid_input(
            "run.not_controllable",
            "This Run is no longer active.",
        ));
    }
    if control == RunControl::InterruptTurn {
        let queue = client
            .turn_queue(execution.run.conversation_id)
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if !queue
            .turns
            .iter()
            .any(|turn| turn.run_id == Some(run_id) && turn.state == TurnState::Active)
        {
            return Err(PublicError::invalid_input(
                "run.no_active_turn",
                "This Run does not have an active Turn to interrupt.",
            ));
        }
    }
    let key = ControlKey {
        run_id,
        stop_run: control == RunControl::StopRun,
    };
    let command_id = command_id(&bridge.run_control.control, key)?;
    let run = client
        .control_run(command_id, run_id, control)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    remove_command(&bridge.run_control.control, &key)?;
    Ok(CommandAcceptedView {
        run: run_view(&run),
        control: control_name(control),
        message: match control {
            RunControl::InterruptTurn => {
                "Interrupt requested. The Run will report when the active Turn ends."
            }
            RunControl::StopRun => {
                "Stop requested. The Run will report when its processes have ended."
            }
        },
    })
}

pub(crate) async fn authorize_approval_retry(
    bridge: State<'_, JetBridge>,
    run_id: String,
    review_id: String,
) -> Result<ApprovalRetryView, PublicError> {
    let run_id = parse_id(&run_id, "run.identifier_invalid")?;
    let review_id = parse_id(&review_id, "review.identifier_invalid")?;
    let key = (run_id, review_id);
    let command_id = command_id(&bridge.run_control.approval_retry, key)?;
    bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .authorize_approval_retry(command_id, run_id, review_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    remove_command(&bridge.run_control.approval_retry, &key)?;
    Ok(ApprovalRetryView {
        review_id: review_id.to_string(),
        message: "One exact retry was authorized. Jet did not change the requested action.",
    })
}

fn turn_view(turn: Turn, position: usize, client_id: Uuid) -> TurnView {
    TurnView {
        id: turn.turn_id.to_string(),
        sequence: turn.sequence.to_string(),
        position: position + 1,
        source: source_name(turn.source),
        state: turn_state_name(turn.state),
        run_id: turn.run_id.map(|id| id.to_string()),
        target: if turn.run_id.is_some() {
            "Current Run"
        } else {
            "Next Run"
        },
        withdrawable: turn.client_id == client_id
            && turn.source == TurnSource::User
            && turn.state == TurnState::Queued,
    }
}

fn execution_view(execution: RunExecution) -> RunExecutionView {
    RunExecutionView {
        cursor: execution.cursor.to_string(),
        run: run_view(&execution.run),
        activity: execution.activity.map(activity_name),
        needs_attention: execution.needs_attention,
        termination: execution.termination.map(|termination| TerminationView {
            control: control_name(termination.control),
            stage: termination_stage_name(termination.stage),
            summary: termination_summary(termination.control, termination.stage),
        }),
    }
}

fn source_name(source: TurnSource) -> &'static str {
    match source {
        TurnSource::User => "user",
        TurnSource::Schedule => "schedule",
        TurnSource::AutoContinue => "auto_continue",
    }
}

fn turn_state_name(state: TurnState) -> &'static str {
    match state {
        TurnState::Queued => "queued",
        TurnState::Active => "active",
        TurnState::Completed => "completed",
        TurnState::Superseded => "superseded",
        TurnState::Canceled => "canceled",
        TurnState::Withdrawn => "withdrawn",
        TurnState::Failed => "failed",
        TurnState::OutcomeUnknown => "outcome_unknown",
    }
}

fn activity_name(activity: RunActivity) -> &'static str {
    match activity {
        RunActivity::Working => "working",
        RunActivity::WaitingForUser => "waiting_for_user",
        RunActivity::WaitingForApproval => "waiting_for_approval",
        RunActivity::WaitingForAuth => "waiting_for_auth",
        RunActivity::WaitingForQuota => "waiting_for_quota",
        RunActivity::Reconnecting => "reconnecting",
    }
}

fn control_name(control: RunControl) -> &'static str {
    match control {
        RunControl::InterruptTurn => "interrupt_turn",
        RunControl::StopRun => "stop_run",
    }
}

fn termination_stage_name(stage: TerminationStage) -> &'static str {
    match stage {
        TerminationStage::NativeCancellation => "native_cancellation",
        TerminationStage::Interrupt => "interrupt",
        TerminationStage::Terminate => "terminate",
        TerminationStage::Kill => "kill",
        TerminationStage::Unobserved => "unobserved",
    }
}

fn termination_summary(control: RunControl, stage: TerminationStage) -> &'static str {
    match (control, stage) {
        (RunControl::InterruptTurn, TerminationStage::NativeCancellation) => {
            "The active Turn was interrupted. The Run can accept the next Turn."
        }
        (RunControl::InterruptTurn, _) => {
            "The active Turn required process termination, so this Run ended."
        }
        (RunControl::StopRun, TerminationStage::Unobserved) => {
            "Jet sent every stop signal but could not observe the Run ending."
        }
        (RunControl::StopRun, _) => "The Run stopped and kept its recorded work.",
    }
}

fn parse_id(value: &str, code: &'static str) -> Result<Uuid, PublicError> {
    if value.len() > 40 {
        return Err(PublicError::invalid_input(code, "That item is not valid."));
    }
    Uuid::parse_str(value).map_err(|_| PublicError::invalid_input(code, "That item is not valid."))
}

fn command_id<K>(commands: &Mutex<HashMap<K, Uuid>>, key: K) -> Result<Uuid, PublicError>
where
    K: std::hash::Hash + Eq,
{
    let mut commands = commands.lock().map_err(|_| PublicError::internal())?;
    if let Some(command_id) = commands.get(&key) {
        return Ok(*command_id);
    }
    if commands.len() >= MAX_PENDING_COMMANDS {
        return Err(PublicError::invalid_input(
            "client.too_many_pending_commands",
            "Finish or retry an earlier action before starting another one.",
        ));
    }
    Ok(*commands.entry(key).or_insert_with(Uuid::new_v4))
}

fn remove_command<K>(commands: &Mutex<HashMap<K, Uuid>>, key: &K) -> Result<(), PublicError>
where
    K: std::hash::Hash + Eq,
{
    commands
        .lock()
        .map_err(|_| PublicError::internal())?
        .remove(key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{termination_summary, RunControl, TerminationStage};

    #[test]
    fn control_outcomes_do_not_collapse_interrupt_and_stop() {
        assert!(termination_summary(
            RunControl::InterruptTurn,
            TerminationStage::NativeCancellation
        )
        .contains("accept the next Turn"));
        assert!(
            termination_summary(RunControl::StopRun, TerminationStage::Kill)
                .contains("Run stopped")
        );
    }
}
