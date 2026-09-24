use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use jet_protocol::{
    BaseSelection, CapabilityObservation, Conversation, ConversationList, ConversationSnapshot,
    PageCursor, RetentionPolicy, Run, RunLifecycle, SearchResult, SeedSelection, Turn, TurnSource,
    WorkingTree, WorkingTreeRequest, FENCED_READS_MINOR, SEARCH_MINOR,
};
use serde::Serialize;
use uuid::Uuid;

use super::{
    command_ids::PendingCommands, errors::PublicError, local_store, planes::PlaneId, JetBridge,
};

const SELECTION_FILE: &str = "last-conversation";
const MAX_SELECTION_BYTES: usize = 80;
const MAX_PROMPT_BYTES: usize = 65_536;
const MAX_SEARCH_BYTES: usize = 256;

/// The Command IDs of composer sends, kept only while one's outcome is
/// uncertain. Each is tied to the webview's `attempt`: one Send of the
/// composer, which the webview keeps only while the user retries that Send
/// unchanged. A retry therefore gets the same ID and the Plane's original
/// answer, while any later Send is a new request, even with the same text
/// in the same Project or Conversation: sent under a kept ID it would only
/// replay the old answer (D2).
#[derive(Default)]
pub(crate) struct ConversationCommands {
    /// Per Project: a local create's body is only its Project (the
    /// retention policy, base and seed are fixed), so an uncertain create
    /// retried by the same Send returns the task it may have made.
    create: PendingCommands<Uuid, Uuid>,
    start: PendingCommands<SendTarget, StartRequest>,
    submit: PendingCommands<SendTarget, SubmitRequest>,
}

pub(crate) struct ConversationState {
    selection_path: PathBuf,
    selection: Mutex<Option<(Uuid, PlaneId)>>,
    commands: ConversationCommands,
}

// Command-ID targets include the Plane: the same Conversation UUID on two
// Planes (ADR-0070 transfer) is two different request bodies.
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct SendTarget {
    plane: PlaneId,
    conversation_id: Uuid,
}

#[derive(PartialEq)]
struct StartRequest {
    craft: String,
    prompt: String,
    attempt: Uuid,
}

#[derive(PartialEq)]
struct SubmitRequest {
    prompt: String,
    attempt: Uuid,
}

impl ConversationState {
    pub(crate) fn new(app_data_directory: &Path) -> Self {
        let selection_path = app_data_directory.join(SELECTION_FILE);
        // Bounded: an oversized or unreadable file restores nothing.
        let selection = local_store::read_bounded(&selection_path, MAX_SELECTION_BYTES as u64)
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|value| parse_selection(&value));
        Self {
            selection_path,
            selection: Mutex::new(selection),
            commands: ConversationCommands::default(),
        }
    }

    pub(crate) fn restored_selection(&self) -> Result<Option<(Uuid, PlaneId)>, PublicError> {
        self.selection
            .lock()
            .map(|selection| *selection)
            .map_err(|_| PublicError::internal())
    }

    fn remember(&self, conversation_id: Uuid, plane: PlaneId) -> Result<(), PublicError> {
        let mut selection = self.selection.lock().map_err(|_| PublicError::internal())?;
        if *selection == Some((conversation_id, plane)) {
            return Ok(());
        }
        // ASVS 14.3.3: browser storage receives no Jet content. Native
        // persistence retains only the non-secret last-selection UUID and the
        // opaque Plane handle, written atomically and owner-only.
        write_atomically(
            &self.selection_path,
            format!("{conversation_id}\n{plane}\n").as_bytes(),
        )
        .map_err(|_| PublicError::internal())?;
        *selection = Some((conversation_id, plane));
        Ok(())
    }
}

/// `<uuid>\n<planeId>\n`, or the older single-UUID form, which names the
/// local Plane. Anything else restores nothing.
fn parse_selection(value: &str) -> Option<(Uuid, PlaneId)> {
    if value.len() > MAX_SELECTION_BYTES {
        return None;
    }
    let mut lines = value.lines();
    let conversation = Uuid::parse_str(lines.next()?.trim()).ok()?;
    let plane = match lines.next() {
        None => PlaneId::Local,
        Some(line) => PlaneId::parse(line.trim()).ok()?,
    };
    if lines.any(|line| !line.trim().is_empty()) {
        return None;
    }
    Some((conversation, plane))
}

fn write_atomically(destination: &Path, contents: &[u8]) -> std::io::Result<()> {
    let directory = destination.parent().unwrap_or_else(|| Path::new("."));
    let temporary = directory.join(format!(".{SELECTION_FILE}.{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = options.open(&temporary).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, destination)
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationPageView {
    plane_id: String,
    cursor: String,
    conversations: Vec<ConversationRowView>,
    next_page: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationRowView {
    plane_id: String,
    id: String,
    revision: Option<String>,
    title: String,
    created_at_unix_ms: String,
    project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationDetailView {
    conversation: ConversationRowView,
    cursor: String,
    workspace_id: Option<String>,
    workspace_root: Option<String>,
    runs: Vec<RunView>,
    /// Disclosed read-only in the task header; no Command changes it.
    retention: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunView {
    id: String,
    conversation_id: String,
    revision: String,
    lifecycle: &'static str,
    title: String,
    created_at_unix_ms: String,
    ended_at_unix_ms: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResultView {
    plane_id: String,
    cursor: String,
    indexed_through: String,
    hits: Vec<SearchHitView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHitView {
    conversation_id: String,
    sequence: String,
    field: &'static str,
    excerpt: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartResultView {
    run: RunView,
    prompt: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnResultView {
    id: String,
    sequence: String,
    state: &'static str,
    prompt: String,
}

pub(crate) async fn load_conversations(
    bridge: &JetBridge,
    plane_id: Option<String>,
    next_page: Option<String>,
) -> Result<ConversationPageView, PublicError> {
    let next_page = next_page
        .map(|cursor| parse_id(&cursor, "conversation.page_invalid").map(PageCursor))
        .transpose()?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    let plane = binding.plane;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let page = match next_page {
            Some(cursor) => {
                let page = client.query(client.next_conversations(cursor)).await;
                if page.is_ok() {
                    bridge.planes.observe_success(plane, FENCED_READS_MINOR);
                }
                page
            }
            None => client.query(client.conversations()).await,
        }
        .map_err(|error| PublicError::from_client(&error))?;
        Ok(page_view(plane, page))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn search_conversations(
    bridge: &JetBridge,
    plane_id: Option<String>,
    text: String,
) -> Result<SearchResultView, PublicError> {
    validate_search(&text)?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    let plane = binding.plane;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let result = client
            .query(client.search(&text))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        bridge.planes.observe_success(plane, SEARCH_MINOR);
        Ok(search_view(plane, result))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn load_conversation(
    bridge: &JetBridge,
    conversation_id: String,
    plane_id: Option<String>,
) -> Result<ConversationDetailView, PublicError> {
    let id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    let plane = binding.plane;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let snapshot = client
            .query(client.conversation(id))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if snapshot.conversation.conversation_id != id {
            return Err(PublicError::internal());
        }
        bridge.conversations.remember(id, plane)?;
        Ok(detail_view(plane, snapshot))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn create_conversation(
    bridge: &JetBridge,
    project_id: String,
    attempt: String,
) -> Result<ConversationRowView, PublicError> {
    let project_id = parse_id(&project_id, "project.identifier_invalid")?;
    let attempt = parse_id(&attempt, "conversation.attempt_invalid")?;
    // New tasks stay on this computer in Wave 3.1: the "Runs on" chooser for
    // remote Planes is deferred client work.
    let local = bridge.local();
    let client = local
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let projects = client
        .query(client.projects())
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    if !projects
        .projects
        .iter()
        .any(|project| project.project_id == project_id)
    {
        return Err(PublicError::invalid_input(
            "project.not_found",
            "Choose a Project that is still registered on this Plane.",
        ));
    }
    // ASVS 2.2.2 and 8.3.1: the native boundary resolves a typed Project
    // and sends only the fixed managed-Workspace variant.
    let commands = &bridge.conversations.commands.create;
    let command_id = commands.id(project_id, attempt)?;
    let outcome = client
        .command(client.create_conversation_in(
            command_id,
            RetentionPolicy::Retain,
            WorkingTreeRequest::Workspace {
                project_id,
                base: BaseSelection::Head,
                seed: SeedSelection::None,
            },
        ))
        .await;
    // A refused create is receipted: keeping its ID would replay the
    // refusal for every later "New task" in this Project (D2).
    commands.settle(&project_id, command_id, &outcome)?;
    let conversation = outcome.map_err(|error| PublicError::from_client(&error))?;
    bridge
        .conversations
        .remember(conversation.conversation_id, PlaneId::Local)?;
    Ok(row_view(PlaneId::Local, &conversation))
}

pub(crate) async fn start_run(
    bridge: &JetBridge,
    conversation_id: String,
    craft: String,
    prompt: String,
    attempt: String,
    plane_id: Option<String>,
) -> Result<StartResultView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let attempt = parse_id(&attempt, "conversation.attempt_invalid")?;
    validate_prompt(&prompt)?;
    validate_token(&craft, "craft.identifier_invalid")?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let capabilities = client
            .query(client.capabilities(CapabilityObservation::Fresh))
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if !capabilities
            .crafts
            .iter()
            .any(|installed| installed.craft_id == craft)
        {
            return Err(PublicError::invalid_input(
                "craft.unavailable",
                "Choose a Craft that is installed on this Plane.",
            ));
        }
        let target = SendTarget {
            plane: binding.plane,
            conversation_id,
        };
        let request = StartRequest {
            craft: craft.clone(),
            prompt: prompt.clone(),
            attempt,
        };
        let commands = &bridge.conversations.commands.start;
        let command_id = commands.id(target, request)?;
        let outcome = client
            .command(client.start_run(command_id, conversation_id, &craft, &prompt))
            .await;
        commands.settle(&target, command_id, &outcome)?;
        let run = outcome.map_err(|error| PublicError::from_client(&error))?;
        Ok(StartResultView {
            run: run_view(&run),
            prompt: bounded_text(&prompt, MAX_PROMPT_BYTES, "Task submitted"),
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

pub(crate) async fn submit_turn(
    bridge: &JetBridge,
    conversation_id: String,
    prompt: String,
    attempt: String,
    plane_id: Option<String>,
) -> Result<TurnResultView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let attempt = parse_id(&attempt, "conversation.attempt_invalid")?;
    validate_prompt(&prompt)?;
    let (binding, plane_client) = bridge.plane(plane_id.as_deref())?;
    async {
        let target = SendTarget {
            plane: binding.plane,
            conversation_id,
        };
        let client = plane_client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let commands = &bridge.conversations.commands.submit;
        let command_id = commands.id(
            target,
            SubmitRequest {
                prompt: prompt.clone(),
                attempt,
            },
        )?;
        let outcome = client
            .command(client.submit_turn(command_id, conversation_id, TurnSource::User, &prompt))
            .await;
        // `turn.queue_full` is receipted: with its ID kept, the same prompt
        // would replay "queue full" after the queue drained (D2).
        commands.settle(&target, command_id, &outcome)?;
        let turn = outcome.map_err(|error| PublicError::from_client(&error))?;
        Ok(turn_view(turn, prompt))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

fn page_view(plane: PlaneId, page: ConversationList) -> ConversationPageView {
    ConversationPageView {
        plane_id: plane.to_string(),
        cursor: page.cursor.to_string(),
        conversations: page
            .conversations
            .iter()
            .map(|conversation| row_view(plane, conversation))
            .collect(),
        next_page: page.next_page.map(|cursor| cursor.0.to_string()),
    }
}

fn detail_view(plane: PlaneId, snapshot: ConversationSnapshot) -> ConversationDetailView {
    let workspace_id = snapshot
        .workspace
        .as_ref()
        .map(|workspace| workspace.workspace_id.to_string());
    ConversationDetailView {
        conversation: row_view(plane, &snapshot.conversation),
        cursor: snapshot.cursor.to_string(),
        workspace_id,
        workspace_root: snapshot
            .workspace
            .map(|workspace| bounded_text(&workspace.root, 4_096, "Workspace")),
        runs: snapshot.runs.iter().map(run_view).collect(),
        retention: retention_name(snapshot.conversation.retention),
    }
}

fn retention_name(policy: RetentionPolicy) -> &'static str {
    match policy {
        RetentionPolicy::Retain => "retain",
        RetentionPolicy::ForgetAfterFinalRun => "forget_after_final_run",
    }
}

fn row_view(plane: PlaneId, conversation: &Conversation) -> ConversationRowView {
    ConversationRowView {
        plane_id: plane.to_string(),
        id: conversation.conversation_id.to_string(),
        revision: conversation.revision.map(|value| value.to_string()),
        title: conversation_title(conversation),
        created_at_unix_ms: conversation.created_at_unix_ms.to_string(),
        project_id: match conversation.working_tree {
            Some(
                WorkingTree::Workspace { project_id } | WorkingTree::LocalCheckout { project_id },
            ) => Some(project_id.to_string()),
            Some(WorkingTree::NoProject) | None => None,
        },
    }
}

/// The bounded title a Conversation is presented with, as in Recent.
pub(super) fn conversation_title(conversation: &Conversation) -> String {
    conversation
        .name
        .as_ref()
        .map(|name| bounded_text(&name.value, 256, "Untitled task"))
        .unwrap_or_else(|| "Untitled task".into())
}

pub(crate) fn run_view(run: &Run) -> RunView {
    RunView {
        id: run.run_id.to_string(),
        conversation_id: run.conversation_id.to_string(),
        revision: run.revision.to_string(),
        lifecycle: lifecycle_name(run.lifecycle),
        title: run
            .name
            .as_ref()
            .map(|name| bounded_text(&name.value, 256, "Run"))
            .unwrap_or_else(|| "Run".into()),
        created_at_unix_ms: run.created_at_unix_ms.to_string(),
        ended_at_unix_ms: run.ended_at_unix_ms.map(|value| value.to_string()),
    }
}

fn search_view(plane: PlaneId, result: SearchResult) -> SearchResultView {
    SearchResultView {
        plane_id: plane.to_string(),
        cursor: result.cursor.to_string(),
        indexed_through: result.indexed_through.to_string(),
        hits: result
            .hits
            .into_iter()
            .map(|hit| SearchHitView {
                conversation_id: hit.conversation_id.to_string(),
                sequence: hit.sequence.to_string(),
                field: match hit.field {
                    jet_protocol::SearchField::Name => "name",
                    jet_protocol::SearchField::Path => "path",
                    jet_protocol::SearchField::Branch => "branch",
                },
                excerpt: bounded_text(&hit.excerpt, 512, "Matching task"),
            })
            .collect(),
    }
}

fn turn_view(turn: Turn, prompt: String) -> TurnResultView {
    TurnResultView {
        id: turn.turn_id.to_string(),
        sequence: turn.sequence.to_string(),
        state: match turn.state {
            jet_protocol::TurnState::Queued => "queued",
            jet_protocol::TurnState::Active => "active",
            jet_protocol::TurnState::Completed => "completed",
            jet_protocol::TurnState::Superseded => "superseded",
            jet_protocol::TurnState::Canceled => "canceled",
            jet_protocol::TurnState::Withdrawn => "withdrawn",
            jet_protocol::TurnState::Failed => "failed",
            jet_protocol::TurnState::OutcomeUnknown => "outcome_unknown",
        },
        prompt: bounded_text(&prompt, MAX_PROMPT_BYTES, "Task submitted"),
    }
}

fn lifecycle_name(lifecycle: RunLifecycle) -> &'static str {
    match lifecycle {
        RunLifecycle::Created => "created",
        RunLifecycle::Starting => "starting",
        RunLifecycle::Active => "active",
        RunLifecycle::Stopping => "stopping",
        RunLifecycle::Completed => "completed",
        RunLifecycle::Failed => "failed",
        RunLifecycle::Canceled => "canceled",
        RunLifecycle::Lost => "lost",
    }
}

fn validate_prompt(prompt: &str) -> Result<(), PublicError> {
    if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(PublicError::invalid_input(
            "turn.invalid_prompt",
            "Enter a task between 1 and 65,536 UTF-8 bytes.",
        ));
    }
    Ok(())
}

fn validate_search(text: &str) -> Result<(), PublicError> {
    let terms = text.split_whitespace().count();
    if text.trim().is_empty() || text.len() > MAX_SEARCH_BYTES || terms > 16 {
        return Err(PublicError::invalid_input(
            "search.invalid_text",
            "Search for 1 to 16 terms using at most 256 UTF-8 bytes.",
        ));
    }
    Ok(())
}

fn validate_token(value: &str, code: &'static str) -> Result<(), PublicError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(PublicError::invalid_input(
            code,
            "That selection is not valid.",
        ));
    }
    Ok(())
}

fn parse_id(value: &str, code: &'static str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| PublicError::invalid_input(code, "That item is not valid."))
}

fn bounded_text(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control) {
        fallback.into()
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use jet_protocol::{
        Actor, ClientMessage, CommandRequest, CommandResponse, Conversation, ErrorCategory,
        Project, ProjectList, QueryRequest, QueryResponse, RetentionPolicy, Turn, TurnSource,
        TurnState,
    };

    use super::{
        create_conversation, parse_selection, retention_name, submit_turn, validate_prompt,
        validate_search, validate_token, ConversationState, SendTarget, StartRequest,
    };
    use crate::jet::{
        command_ids::PendingCommands,
        fake_plane::{
            answer, command_id as sent_id, complete, exchange, local_plane, next, refuse,
            remote_plane, wire, FakePlane,
        },
        planes::PlaneId,
    };

    const TASK: Uuid = Uuid::from_u128(0xc0);
    const PROJECT: Uuid = Uuid::from_u128(0xc1);
    /// One Send of the composer, and a later one.
    const SEND: Uuid = Uuid::from_u128(0xa1);
    const LATER_SEND: Uuid = Uuid::from_u128(0xa2);

    /// Serves one `submit_turn` and answers its Command with `reply`, or
    /// admits the Turn. Returns the Command ID the shell sent.
    async fn serve_submit(fake: &FakePlane, reply: Option<(ErrorCategory, &'static str)>) -> Uuid {
        let (mut reader, mut writer) = fake.accept().await;
        let (stream, message) = next(&mut reader).await;
        assert!(matches!(
            &message,
            ClientMessage::Command {
                command: CommandRequest::SubmitTurn { conversation_id, prompt, .. },
                ..
            } if *conversation_id == TASK && prompt == "Ship it"
        ));
        match reply {
            Some((category, code)) => {
                refuse(&mut writer, stream, &message, wire(category, code)).await;
            }
            None => {
                let turn = Turn {
                    turn_id: Uuid::from_u128(0xc2),
                    sequence: 2,
                    client_id: Uuid::from_u128(7),
                    source: TurnSource::User,
                    state: TurnState::Queued,
                    run_id: None,
                };
                complete(
                    &mut writer,
                    stream,
                    &message,
                    CommandResponse::TurnAdmitted { turn },
                )
                .await;
            }
        }
        sent_id(&message)
    }

    async fn submit(
        fake: &FakePlane,
        reply: Option<(ErrorCategory, &'static str)>,
    ) -> (Result<(), String>, Option<Uuid>) {
        submit_as(fake, SEND, reply).await
    }

    /// `submit` as the composer Send `attempt`.
    async fn submit_as(
        fake: &FakePlane,
        attempt: Uuid,
        reply: Option<(ErrorCategory, &'static str)>,
    ) -> (Result<(), String>, Option<Uuid>) {
        let (outcome, id) = exchange(
            submit_turn(
                fake.bridge(),
                TASK.to_string(),
                "Ship it".into(),
                attempt.to_string(),
                Some(fake.plane_id.clone()),
            ),
            serve_submit(fake, reply),
        )
        .await;
        (outcome.map(|_| ()).map_err(|error| error.code), id)
    }

    /// D2: `turn.queue_full` is receipted. Once the queue drains, the same
    /// prompt is a new request instead of a replay of "queue full"; an
    /// outcome-unknown answer keeps the ID for an exact retry.
    #[tokio::test]
    async fn a_refused_turn_releases_its_id_and_an_unknown_one_keeps_it() {
        let fake = remote_plane();
        let (outcome, full) =
            submit(&fake, Some((ErrorCategory::Conflict, "turn.queue_full"))).await;
        assert_eq!(outcome, Err("turn.queue_full".into()));

        let (outcome, unknown) = submit(
            &fake,
            Some((ErrorCategory::OutcomeUnknown, "command.outcome_unknown")),
        )
        .await;
        assert!(unknown.is_some());
        assert_ne!(unknown, full, "the refusal is not replayed");
        assert_eq!(outcome, Err("command.outcome_unknown".into()));

        let (outcome, retried) = submit(&fake, None).await;
        assert_eq!(retried, unknown, "an unknown outcome is retried exactly");
        assert_eq!(outcome, Ok(()));
        assert!(fake.bridge().conversations.commands.submit.is_empty());
    }

    /// D2: an uncertain Turn that the Plane admitted keeps its ID for the
    /// retry of that Send only. A later Send of the same text is new work:
    /// under the kept ID the Plane would replay the old admission and the
    /// new message would never be queued.
    #[tokio::test]
    async fn a_later_send_of_the_same_prompt_is_not_a_replay() {
        let fake = remote_plane();
        let unknown = Some((ErrorCategory::OutcomeUnknown, "command.outcome_unknown"));
        let (_, first) = submit_as(&fake, SEND, unknown).await;
        let (_, retried) = submit_as(&fake, SEND, unknown).await;
        assert_eq!(retried, first, "the same Send retries exactly");

        let (outcome, later) = submit_as(&fake, LATER_SEND, None).await;
        assert_eq!(outcome, Ok(()));
        assert!(later.is_some());
        assert_ne!(later, first, "a later Send never replays the kept ID");
        assert!(fake.bridge().conversations.commands.submit.is_empty());
    }

    /// D2: a refused create does not block "New task" in its Project.
    #[tokio::test]
    async fn a_refused_create_does_not_block_new_tasks_in_the_project() {
        let fake = local_plane();
        let serve = |refusal: Option<&'static str>| {
            let fake = &fake;
            async move {
                let (mut reader, mut writer) = fake.accept().await;
                let (stream, message) = next(&mut reader).await;
                assert!(matches!(
                    message,
                    ClientMessage::Query {
                        query: QueryRequest::Projects,
                        ..
                    }
                ));
                let projects = ProjectList {
                    cursor: 1,
                    projects: vec![Project {
                        project_id: PROJECT,
                        root: "/work/jet".into(),
                        registered_by: Actor::InteractiveClient {
                            client_id: Uuid::from_u128(7),
                        },
                        registered_at_unix_ms: 1,
                    }],
                };
                answer(
                    &mut writer,
                    stream,
                    &message,
                    QueryResponse::Projects(projects),
                )
                .await;
                let (stream, message) = next(&mut reader).await;
                assert!(matches!(
                    message,
                    ClientMessage::Command {
                        command: CommandRequest::CreateConversation { .. },
                        ..
                    }
                ));
                match refusal {
                    Some(code) => {
                        let category = if code == "command.outcome_unknown" {
                            ErrorCategory::OutcomeUnknown
                        } else {
                            ErrorCategory::Unavailable
                        };
                        refuse(&mut writer, stream, &message, wire(category, code)).await;
                    }
                    None => {
                        let conversation = Conversation {
                            conversation_id: TASK,
                            revision: Some(1),
                            retention: RetentionPolicy::Retain,
                            working_tree: None,
                            origin: None,
                            name: None,
                            created_at_unix_ms: 1,
                        };
                        complete(
                            &mut writer,
                            stream,
                            &message,
                            CommandResponse::ConversationCreated(conversation),
                        )
                        .await;
                    }
                }
                sent_id(&message)
            }
        };
        let create = |attempt: Uuid| {
            create_conversation(fake.bridge(), PROJECT.to_string(), attempt.to_string())
        };
        let (refused, first) = exchange(create(SEND), serve(Some("storage.disk_pressure"))).await;
        assert_eq!(refused.unwrap_err().code, "storage.disk_pressure");
        let (created, second) = exchange(create(SEND), serve(None)).await;
        assert_eq!(created.unwrap().id, TASK.to_string());
        assert!(second.is_some());
        assert_ne!(second, first, "the refused create is not replayed");

        // D2: an uncertain create is retried by its own Send only. "New
        // task" sent again later creates a task instead of reopening the
        // one the uncertain create may have made.
        let (unknown, third) = exchange(create(SEND), serve(Some("command.outcome_unknown"))).await;
        assert_eq!(unknown.unwrap_err().code, "command.outcome_unknown");
        let (_, retried) = exchange(create(SEND), serve(Some("command.outcome_unknown"))).await;
        assert_eq!(retried, third, "the same Send retries exactly");
        let (created, later) = exchange(create(LATER_SEND), serve(None)).await;
        assert!(created.is_ok());
        assert!(later.is_some());
        assert_ne!(later, third, "a later Send never replays the kept ID");
        assert!(fake.bridge().conversations.commands.create.is_empty());
    }

    #[test]
    fn selection_persists_its_plane_atomically_and_accepts_the_old_form() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = Uuid::from_u128(4);
        let remote = PlaneId::Remote(Uuid::from_u128(9));
        let state = ConversationState::new(directory.path());
        assert_eq!(state.restored_selection().unwrap(), None);
        state.remember(conversation, remote).unwrap();
        let reloaded = ConversationState::new(directory.path());
        assert_eq!(
            reloaded.restored_selection().unwrap(),
            Some((conversation, remote))
        );
        let written = std::fs::read_to_string(directory.path().join("last-conversation")).unwrap();
        assert_eq!(written, format!("{conversation}\n{}\n", Uuid::from_u128(9)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join("last-conversation"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "no temporary file is left behind"
        );

        assert_eq!(
            parse_selection(&format!("{conversation}\n")),
            Some((conversation, PlaneId::Local))
        );
        assert_eq!(
            parse_selection(&format!("{conversation}\nlocal\n")),
            Some((conversation, PlaneId::Local))
        );
        assert_eq!(
            parse_selection(&format!("{conversation}\n/tmp/jetd.sock\n")),
            None
        );
        assert_eq!(
            parse_selection(&format!("{conversation}\nlocal\nextra\n")),
            None
        );
        assert_eq!(parse_selection(&"x".repeat(81)), None);
    }

    #[test]
    fn command_ids_for_the_same_conversation_differ_across_planes() {
        let commands = PendingCommands::default();
        let target = |plane| SendTarget {
            plane,
            conversation_id: Uuid::from_u128(4),
        };
        let request = || StartRequest {
            craft: "codex".into(),
            prompt: "Ship it".into(),
            attempt: SEND,
        };
        let local = commands.id(target(PlaneId::Local), request()).unwrap();
        let remote = commands
            .id(target(PlaneId::Remote(Uuid::from_u128(9))), request())
            .unwrap();
        assert_ne!(local, remote);
        assert_eq!(
            commands.id(target(PlaneId::Local), request()).unwrap(),
            local
        );
    }

    #[test]
    fn webview_inputs_are_bounded_before_protocol_use() {
        assert!(validate_prompt("Ship it").is_ok());
        assert!(validate_prompt("").is_err());
        assert!(validate_prompt("   \n").is_err());
        assert!(validate_prompt(&"x".repeat(65_537)).is_err());
        assert!(validate_search("project task").is_ok());
        assert!(validate_search(&"x".repeat(257)).is_err());
        assert!(validate_token("codex_default", "craft.invalid").is_ok());
        assert!(validate_token("../../craft", "craft.invalid").is_err());
    }

    #[test]
    fn the_detail_view_discloses_the_retention_policy() {
        assert_eq!(retention_name(RetentionPolicy::Retain), "retain");
        assert_eq!(
            retention_name(RetentionPolicy::ForgetAfterFinalRun),
            "forget_after_final_run"
        );
    }
}
