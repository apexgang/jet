use std::{collections::HashMap, sync::Mutex};

use jet_protocol::{
    RunActivity, RunControl, RunExecution, RunLifecycle, TerminationStage, Turn, TurnSource,
    TurnState, APPROVAL_RETRY_MINOR, TURN_QUEUE_MINOR,
};
use serde::Serialize;
use uuid::Uuid;

use super::{
    command_ids::{settle_command, PendingCommands},
    conversations::{run_view, RunView},
    errors::PublicError,
    planes::PlaneId,
    JetBridge,
};

const MAX_PENDING_COMMANDS: usize = 256;
const MAX_QUEUE_ENTRIES: usize = 128;
const MAX_PROMPT_BYTES: usize = 65_536;

#[derive(Default)]
pub(crate) struct RunControlState {
    withdraw: Mutex<HashMap<(PlaneId, Uuid, Uuid), Uuid>>,
    /// Interrupt is kept for the active Turn it was sent against: the
    /// Command's body names only the Run, so a later Turn's Interrupt under
    /// the same ID would replay the old acceptance and leave that Turn
    /// running. Stop is kept for its Run, which it ends.
    control: PendingCommands<ControlKey, Option<Uuid>>,
    approval_retry: Mutex<HashMap<(PlaneId, Uuid, Uuid), Uuid>>,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct ControlKey {
    plane: PlaneId,
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
    bridge: &JetBridge,
    conversation_id: String,
    run_id: Option<String>,
    plane_id: Option<String>,
) -> Result<RunSupervisionView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let run_id = run_id
        .map(|value| parse_id(&value, "run.identifier_invalid"))
        .transpose()?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let queue = client
            .query(client.turn_queue(conversation_id))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge
            .planes
            .observe_success(binding.plane, TURN_QUEUE_MINOR);
        let execution = match run_id {
            Some(run_id) => {
                let execution = client
                    .query(client.run_execution(run_id))
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
                .map(|(index, turn)| turn_view(turn, index, plane_client.client_id()))
                .collect(),
            execution,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn withdraw_turn(
    bridge: &JetBridge,
    conversation_id: String,
    turn_id: String,
    plane_id: Option<String>,
) -> Result<TurnView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let turn_id = parse_id(&turn_id, "turn.identifier_invalid")?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let queue = client
            .query(client.turn_queue(conversation_id))
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
        if candidate.client_id != plane_client.client_id()
            || candidate.source != TurnSource::User
            || candidate.state != TurnState::Queued
        {
            return Err(PublicError::invalid_input(
                "turn.withdraw_denied",
                "Only your own queued user Turn can be withdrawn.",
            ));
        }
        let key = (binding.plane, conversation_id, turn_id);
        let command_id = command_id(&bridge.run_control.withdraw, key)?;
        let outcome = client
            .command(client.withdraw_turn(command_id, conversation_id, turn_id))
            .await;
        settle_command(&bridge.run_control.withdraw, &key, &outcome)?;
        let turn = outcome.map_err(|error| PublicError::from_client(&error))?;
        Ok(turn_view(turn, position, plane_client.client_id()))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn interrupt_turn(
    bridge: &JetBridge,
    run_id: String,
    plane_id: Option<String>,
) -> Result<CommandAcceptedView, PublicError> {
    control_run(bridge, run_id, RunControl::InterruptTurn, plane_id).await
}

pub(crate) async fn stop_run(
    bridge: &JetBridge,
    run_id: String,
    plane_id: Option<String>,
) -> Result<CommandAcceptedView, PublicError> {
    control_run(bridge, run_id, RunControl::StopRun, plane_id).await
}

async fn control_run(
    bridge: &JetBridge,
    run_id: String,
    control: RunControl,
    plane_id: Option<String>,
) -> Result<CommandAcceptedView, PublicError> {
    let run_id = parse_id(&run_id, "run.identifier_invalid")?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    let plane = binding.plane;
    let commands = &bridge.run_control.control;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let execution = client
            .query(client.run_execution(run_id))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if execution.run.lifecycle != RunLifecycle::Active {
            // An uncertain Interrupt or Stop of this Run has applied or can
            // no longer apply: its kept ID must not outlive it (D1).
            commands.forget(|key| key.plane == plane && key.run_id == run_id)?;
            return Err(PublicError::invalid_input(
                "run.not_controllable",
                "This Run is no longer active.",
            ));
        }
        let turn = if control == RunControl::InterruptTurn {
            let queue = client
                .query(client.turn_queue(execution.run.conversation_id))
                .await
                .map_err(|error| PublicError::from_client(&error))?;
            let Some(turn) = queue
                .turns
                .iter()
                .find(|turn| turn.run_id == Some(run_id) && turn.state == TurnState::Active)
            else {
                commands
                    .forget(|key| key.plane == plane && key.run_id == run_id && !key.stop_run)?;
                return Err(PublicError::invalid_input(
                    "run.no_active_turn",
                    "This Run does not have an active Turn to interrupt.",
                ));
            };
            Some(turn.turn_id)
        } else {
            None
        };
        let key = ControlKey {
            plane,
            run_id,
            stop_run: control == RunControl::StopRun,
        };
        // An Interrupt of a later Turn replaces an uncertain one of an
        // earlier Turn instead of replaying its acceptance (D1).
        let command_id = commands.id(key, turn)?;
        // A refusal such as `run.no_active_turn` (the Turn ended after the
        // check above) is receipted under this ID: it is dropped with the
        // refusal, so a later Interrupt or Stop is a new request (D1).
        let outcome = client
            .command(client.control_run(command_id, run_id, control))
            .await;
        commands.settle(&key, command_id, &outcome)?;
        let run = outcome.map_err(|error| PublicError::from_client(&error))?;
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
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn authorize_approval_retry(
    bridge: &JetBridge,
    run_id: String,
    review_id: String,
    plane_id: Option<String>,
) -> Result<ApprovalRetryView, PublicError> {
    let run_id = parse_id(&run_id, "run.identifier_invalid")?;
    let review_id = parse_id(&review_id, "review.identifier_invalid")?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    async {
        let key = (binding.plane, run_id, review_id);
        let command_id = command_id(&bridge.run_control.approval_retry, key)?;
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let outcome = client
            .command(client.authorize_approval_retry(command_id, run_id, review_id))
            .await;
        settle_command(&bridge.run_control.approval_retry, &key, &outcome)?;
        outcome.map_err(|error| PublicError::from_client(&error))?;
        bridge
            .planes
            .observe_success(binding.plane, APPROVAL_RETRY_MINOR);
        Ok(ApprovalRetryView {
            review_id: review_id.to_string(),
            message: "One exact retry was authorized. Jet did not change the requested action.",
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
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

#[cfg(test)]
mod tests {
    use jet_protocol::{
        ClientMessage, CommandRequest, CommandResponse, ErrorCategory, QueryRequest, QueryResponse,
        Run, RunExecution, RunLifecycle, TurnQueue,
    };
    use uuid::Uuid;

    use super::{interrupt_turn, stop_run, termination_summary, RunControl, TerminationStage};
    use crate::jet::{
        deadline::COMMAND_DEADLINE,
        fake_plane::{
            answer, command_id, complete, next, refuse, remote_plane, wire, FakePlane, Reader,
            Writer,
        },
    };

    const RUN: Uuid = Uuid::from_u128(0x70);
    const CONVERSATION: Uuid = Uuid::from_u128(0x71);
    const TURN: Uuid = Uuid::from_u128(0x72);

    fn run() -> Run {
        Run {
            run_id: RUN,
            conversation_id: CONVERSATION,
            revision: 3,
            lifecycle: RunLifecycle::Active,
            name: None,
            created_at_unix_ms: 1,
            ended_at_unix_ms: None,
        }
    }

    fn execution(lifecycle: RunLifecycle) -> RunExecution {
        RunExecution {
            needs_attention: false,
            no_visa: None,
            visa: None,
            cursor: 9,
            run: Run { lifecycle, ..run() },
            activity: None,
            processes: Vec::new(),
            native_conversation: None,
            exit_code: None,
            termination: None,
        }
    }

    /// How the scripted Plane answers the Interrupt Command itself.
    enum Reply {
        Refuse(ErrorCategory, &'static str),
        /// `run.revision_conflict` with the Run's safe state.
        Conflict,
        Accept,
        /// Never answers; the test moves the clock past the deadline.
        Hang,
    }

    /// Serves one Interrupt: the Run is active with an active Turn, then the
    /// Command gets `reply`. Returns the Command ID the shell sent, and a
    /// hung connection to keep open until the shell gives up.
    async fn serve(fake: &FakePlane, reply: Reply) -> (Uuid, Option<(Reader, Writer)>) {
        serve_turn(fake, Some(TURN), reply).await
    }

    /// Accepts one connection and answers its read of the Run with
    /// `lifecycle`.
    async fn serve_execution(fake: &FakePlane, lifecycle: RunLifecycle) -> (Reader, Writer) {
        let (mut reader, mut writer) = fake.accept().await;
        let (stream, message) = next(&mut reader).await;
        assert!(matches!(
            message,
            ClientMessage::Query {
                query: QueryRequest::RunExecution { run_id: RUN },
                ..
            }
        ));
        answer(
            &mut writer,
            stream,
            &message,
            QueryResponse::RunExecution(execution(lifecycle)),
        )
        .await;
        (reader, writer)
    }

    /// `serve` with `active` as the Run's active Turn. Without one the
    /// queue shows the earlier Turn ended, and no Command is expected.
    async fn serve_turn(
        fake: &FakePlane,
        active: Option<Uuid>,
        reply: Reply,
    ) -> (Uuid, Option<(Reader, Writer)>) {
        let (mut reader, mut writer) = serve_execution(fake, RunLifecycle::Active).await;
        let (stream, message) = next(&mut reader).await;
        let turn = jet_protocol::Turn {
            turn_id: active.unwrap_or(TURN),
            sequence: 1,
            client_id: Uuid::from_u128(7),
            source: jet_protocol::TurnSource::User,
            state: if active.is_some() {
                jet_protocol::TurnState::Active
            } else {
                jet_protocol::TurnState::Canceled
            },
            run_id: Some(RUN),
        };
        answer(
            &mut writer,
            stream,
            &message,
            QueryResponse::TurnQueue(TurnQueue {
                cursor: 9,
                turns: vec![turn],
            }),
        )
        .await;
        if active.is_none() {
            return (Uuid::nil(), None);
        }
        let (stream, message) = next(&mut reader).await;
        assert!(matches!(
            message,
            ClientMessage::Command {
                command: CommandRequest::InterruptTurn { run_id: RUN },
                ..
            }
        ));
        let id = command_id(&message);
        match reply {
            Reply::Refuse(category, code) => {
                refuse(&mut writer, stream, &message, wire(category, code)).await;
            }
            Reply::Conflict => {
                let mut conflict = wire(ErrorCategory::Conflict, "run.revision_conflict");
                conflict.revision_conflict = Some(jet_protocol::RevisionConflict {
                    current_revision: 4,
                    safe_state: jet_protocol::ConflictState::Run {
                        run: Run {
                            revision: 4,
                            lifecycle: RunLifecycle::Stopping,
                            ..run()
                        },
                    },
                });
                refuse(&mut writer, stream, &message, conflict).await;
            }
            Reply::Accept => {
                complete(
                    &mut writer,
                    stream,
                    &message,
                    CommandResponse::RunControlAccepted {
                        run: run(),
                        control: RunControl::InterruptTurn,
                    },
                )
                .await;
            }
            Reply::Hang => {
                fake.advance(COMMAND_DEADLINE);
                return (id, Some((reader, writer)));
            }
        }
        (id, None)
    }

    /// A Revision conflict is a definite refusal: it reaches the webview with
    /// the Run's safe state, and the deliberate retry after the refresh is a
    /// new request that succeeds.
    #[tokio::test]
    async fn a_revision_conflict_carries_the_safe_run_and_a_retry_is_new() {
        let fake = remote_plane();
        let (conflicted, (first, _)) = tokio::join!(
            interrupt_turn(fake.bridge(), RUN.to_string(), Some(fake.plane_id.clone())),
            serve(&fake, Reply::Conflict)
        );
        let error = conflicted.unwrap_err();
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "run.revision_conflict");
        assert_eq!(json["planeId"], fake.plane_id);
        assert_eq!(json["revisionConflict"]["currentRevision"], "4");
        assert_eq!(json["revisionConflict"]["safeState"]["type"], "run");
        assert_eq!(
            json["revisionConflict"]["safeState"]["lifecycle"],
            "stopping"
        );

        let (outcome, retried) = interrupt(&fake, Reply::Accept).await;
        assert_eq!(outcome, Ok("\"interrupt_turn\"".into()));
        assert_ne!(retried, first);
    }

    async fn interrupt(fake: &FakePlane, reply: Reply) -> (Result<String, String>, Uuid) {
        interrupt_of(fake, Some(TURN), reply).await
    }

    /// An Interrupt while `active` is the Run's active Turn.
    async fn interrupt_of(
        fake: &FakePlane,
        active: Option<Uuid>,
        reply: Reply,
    ) -> (Result<String, String>, Uuid) {
        let (outcome, (id, _open)) = tokio::join!(
            interrupt_turn(fake.bridge(), RUN.to_string(), Some(fake.plane_id.clone())),
            serve_turn(fake, active, reply)
        );
        (
            outcome
                .map(|view| serde_json::to_value(view).unwrap()["control"].to_string())
                .map_err(|error| format!("{}:{}", error.category, error.code)),
            id,
        )
    }

    /// D1: an uncertain Interrupt that the Plane applied ends its Turn, so
    /// the retry stops at the pre-check and never settles the ID. The kept ID
    /// must not reach a later Turn's Interrupt: its body names only the Run,
    /// so the Plane would replay the old acceptance and leave the new Turn
    /// running while the UI says it was interrupted.
    #[tokio::test]
    async fn an_uncertain_interrupt_is_never_replayed_for_a_later_turn() {
        let fake = remote_plane();
        let unknown = || Reply::Refuse(ErrorCategory::OutcomeUnknown, "command.outcome_unknown");
        let (_, first) = interrupt_of(&fake, Some(TURN), unknown()).await;

        // The Interrupt applied: no active Turn is left to retry against.
        let (outcome, _) = interrupt_of(&fake, None, Reply::Accept).await;
        assert_eq!(outcome, Err("invalid_input:run.no_active_turn".into()));
        assert!(
            fake.bridge().run_control.control.is_empty(),
            "a Turn that ended takes its kept ID with it"
        );

        let later = Uuid::from_u128(0x73);
        let (outcome, second) = interrupt_of(&fake, Some(later), Reply::Accept).await;
        assert_eq!(outcome, Ok("\"interrupt_turn\"".into()));
        assert_ne!(second, first);

        // Without a pre-check in between, a later Turn still gets its own ID.
        let (_, uncertain) = interrupt_of(&fake, Some(later), unknown()).await;
        let (_, next_turn) = interrupt_of(&fake, Some(Uuid::from_u128(0x74)), Reply::Accept).await;
        assert_ne!(next_turn, uncertain);
        assert!(fake.bridge().run_control.control.is_empty());
    }

    /// D1: an uncertain Stop is forgotten once the Run is no longer active.
    #[tokio::test]
    async fn an_uncertain_stop_is_forgotten_when_the_run_ends() {
        let fake = remote_plane();
        let (outcome, _open) = tokio::join!(
            stop_run(fake.bridge(), RUN.to_string(), Some(fake.plane_id.clone())),
            async {
                let (mut reader, mut writer) = serve_execution(&fake, RunLifecycle::Active).await;
                let (stream, message) = next(&mut reader).await;
                assert!(matches!(
                    message,
                    ClientMessage::Command {
                        command: CommandRequest::StopRun { run_id: RUN },
                        ..
                    }
                ));
                let unknown = wire(ErrorCategory::OutcomeUnknown, "command.outcome_unknown");
                refuse(&mut writer, stream, &message, unknown).await;
                (reader, writer)
            }
        );
        assert_eq!(outcome.unwrap_err().code, "command.outcome_unknown");
        assert!(!fake.bridge().run_control.control.is_empty());

        let (outcome, _open) = tokio::join!(
            stop_run(fake.bridge(), RUN.to_string(), Some(fake.plane_id.clone())),
            serve_execution(&fake, RunLifecycle::Stopping)
        );
        assert_eq!(outcome.unwrap_err().code, "run.not_controllable");
        assert!(fake.bridge().run_control.control.is_empty());
    }

    /// D1: `run.no_active_turn` is receipted under the Command ID. A later
    /// Interrupt must be a new request, while an uncertain one (outcome
    /// unknown, or no answer before the deadline) resends the same ID.
    #[tokio::test]
    async fn a_refused_interrupt_releases_its_id_and_an_uncertain_one_keeps_it() {
        let fake = remote_plane();
        let (outcome, refused) = interrupt(
            &fake,
            Reply::Refuse(ErrorCategory::Conflict, "run.no_active_turn"),
        )
        .await;
        assert_eq!(outcome, Err("conflict:run.no_active_turn".into()));

        let (outcome, unknown) = interrupt(
            &fake,
            Reply::Refuse(ErrorCategory::OutcomeUnknown, "command.outcome_unknown"),
        )
        .await;
        assert_ne!(unknown, refused, "a refusal must not be replayed");
        assert_eq!(
            outcome,
            Err("outcome_unknown:command.outcome_unknown".into())
        );

        let (outcome, hung) = interrupt(&fake, Reply::Hang).await;
        assert_eq!(hung, unknown, "an uncertain outcome is retried exactly");
        assert_eq!(
            outcome,
            Err("outcome_unknown:command.outcome_unknown".into())
        );

        let (outcome, accepted) = interrupt(&fake, Reply::Accept).await;
        assert_eq!(accepted, unknown);
        assert_eq!(outcome, Ok("\"interrupt_turn\"".into()));

        let (_, next_id) = interrupt(&fake, Reply::Accept).await;
        assert_ne!(next_id, accepted, "a settled Command frees its ID");
        assert!(fake.bridge().run_control.control.is_empty());
    }

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
