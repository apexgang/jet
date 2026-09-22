use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use jet_protocol::{
    BaseSelection, CapabilityObservation, Conversation, ConversationList, ConversationSnapshot,
    PageCursor, RetentionPolicy, Run, RunLifecycle, SearchResult, SeedSelection, Turn, TurnSource,
    WorkingTree, WorkingTreeRequest,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use super::{errors::PublicError, JetBridge};

const SELECTION_FILE: &str = "last-conversation";
const MAX_PROMPT_BYTES: usize = 65_536;
const MAX_SEARCH_BYTES: usize = 256;
const MAX_PENDING_COMMANDS: usize = 256;

#[derive(Default)]
pub(crate) struct ConversationCommands {
    create: Mutex<HashMap<Uuid, Uuid>>,
    start: Mutex<HashMap<StartKey, Uuid>>,
    submit: Mutex<HashMap<SubmitKey, Uuid>>,
}

pub(crate) struct ConversationState {
    selection_path: PathBuf,
    selection: Mutex<Option<Uuid>>,
    commands: ConversationCommands,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct StartKey {
    conversation_id: Uuid,
    craft: String,
    prompt: String,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct SubmitKey {
    conversation_id: Uuid,
    prompt: String,
}

impl ConversationState {
    pub(crate) fn new(app_data_directory: &Path) -> Self {
        let selection_path = app_data_directory.join(SELECTION_FILE);
        let selection = fs::read_to_string(&selection_path)
            .ok()
            .filter(|value| value.len() <= 40)
            .and_then(|value| Uuid::parse_str(value.trim()).ok());
        Self {
            selection_path,
            selection: Mutex::new(selection),
            commands: ConversationCommands::default(),
        }
    }

    fn restored_selection(&self) -> Result<Option<Uuid>, PublicError> {
        self.selection
            .lock()
            .map(|selection| *selection)
            .map_err(|_| PublicError::internal())
    }

    fn remember(&self, conversation_id: Uuid) -> Result<(), PublicError> {
        let mut selection = self.selection.lock().map_err(|_| PublicError::internal())?;
        // ASVS 14.3.3: browser storage receives no Jet content. Native
        // persistence retains only the non-secret last-selection UUID.
        fs::write(&self.selection_path, format!("{conversation_id}\n"))
            .map_err(|_| PublicError::internal())?;
        *selection = Some(conversation_id);
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationPageView {
    cursor: String,
    conversations: Vec<ConversationRowView>,
    next_page: Option<String>,
    restored_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationRowView {
    id: String,
    title: String,
    created_at_unix_ms: String,
    project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationDetailView {
    conversation: ConversationRowView,
    cursor: String,
    workspace_root: Option<String>,
    runs: Vec<RunView>,
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
    bridge: State<'_, JetBridge>,
    next_page: Option<String>,
) -> Result<ConversationPageView, PublicError> {
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let page = match next_page {
        Some(cursor) => {
            let cursor = PageCursor(parse_id(&cursor, "conversation.page_invalid")?);
            client.next_conversations(cursor).await
        }
        None => client.conversations().await,
    }
    .map_err(|error| PublicError::from_client(&error))?;
    page_view(&bridge.conversations, page)
}

pub(crate) async fn search_conversations(
    bridge: State<'_, JetBridge>,
    text: String,
) -> Result<SearchResultView, PublicError> {
    validate_search(&text)?;
    let result = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .search(&text)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    Ok(search_view(result))
}

pub(crate) async fn load_conversation(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
) -> Result<ConversationDetailView, PublicError> {
    let id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    let snapshot = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .conversation(id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge.conversations.remember(id)?;
    Ok(detail_view(snapshot))
}

pub(crate) async fn create_conversation(
    bridge: State<'_, JetBridge>,
    project_id: String,
) -> Result<ConversationRowView, PublicError> {
    let project_id = parse_id(&project_id, "project.identifier_invalid")?;
    let command_id = command_id(&bridge.conversations.commands.create, project_id)?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let projects = client
        .projects()
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
    let conversation = client
        .create_conversation_in(
            command_id,
            RetentionPolicy::Retain,
            WorkingTreeRequest::Workspace {
                project_id,
                base: BaseSelection::Head,
                seed: SeedSelection::None,
            },
        )
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    remove_command(&bridge.conversations.commands.create, &project_id)?;
    bridge
        .conversations
        .remember(conversation.conversation_id)?;
    Ok(row_view(&conversation))
}

pub(crate) async fn start_run(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    craft: String,
    prompt: String,
) -> Result<StartResultView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    validate_prompt(&prompt)?;
    validate_token(&craft, "craft.identifier_invalid")?;
    let key = StartKey {
        conversation_id,
        craft: craft.clone(),
        prompt: prompt.clone(),
    };
    let command_id = command_id(&bridge.conversations.commands.start, key.clone())?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let capabilities = client
        .capabilities(CapabilityObservation::Fresh)
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
    let run = client
        .start_run(command_id, conversation_id, &craft, &prompt)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    remove_command(&bridge.conversations.commands.start, &key)?;
    Ok(StartResultView {
        run: run_view(&run),
        prompt: bounded_text(&prompt, MAX_PROMPT_BYTES, "Task submitted"),
    })
}

pub(crate) async fn submit_turn(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    prompt: String,
) -> Result<TurnResultView, PublicError> {
    let conversation_id = parse_id(&conversation_id, "conversation.identifier_invalid")?;
    validate_prompt(&prompt)?;
    let key = SubmitKey {
        conversation_id,
        prompt: prompt.clone(),
    };
    let command_id = command_id(&bridge.conversations.commands.submit, key.clone())?;
    let turn = bridge
        .client
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?
        .submit_turn(command_id, conversation_id, TurnSource::User, &prompt)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    remove_command(&bridge.conversations.commands.submit, &key)?;
    Ok(turn_view(turn, prompt))
}

fn page_view(
    state: &ConversationState,
    page: ConversationList,
) -> Result<ConversationPageView, PublicError> {
    Ok(ConversationPageView {
        cursor: page.cursor.to_string(),
        conversations: page.conversations.iter().map(row_view).collect(),
        next_page: page.next_page.map(|cursor| cursor.0.to_string()),
        restored_id: state.restored_selection()?.map(|id| id.to_string()),
    })
}

fn detail_view(snapshot: ConversationSnapshot) -> ConversationDetailView {
    ConversationDetailView {
        conversation: row_view(&snapshot.conversation),
        cursor: snapshot.cursor.to_string(),
        workspace_root: snapshot
            .workspace
            .map(|workspace| bounded_text(&workspace.root, 4_096, "Workspace")),
        runs: snapshot.runs.iter().map(run_view).collect(),
    }
}

fn row_view(conversation: &Conversation) -> ConversationRowView {
    ConversationRowView {
        id: conversation.conversation_id.to_string(),
        title: conversation
            .name
            .as_ref()
            .map(|name| bounded_text(&name.value, 256, "Untitled task"))
            .unwrap_or_else(|| "Untitled task".into()),
        created_at_unix_ms: conversation.created_at_unix_ms.to_string(),
        project_id: match conversation.working_tree {
            Some(
                WorkingTree::Workspace { project_id } | WorkingTree::LocalCheckout { project_id },
            ) => Some(project_id.to_string()),
            Some(WorkingTree::NoProject) | None => None,
        },
    }
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

fn search_view(result: SearchResult) -> SearchResultView {
    SearchResultView {
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
            "Finish or retry an earlier task before starting another one.",
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

fn bounded_text(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control) {
        fallback.into()
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_prompt, validate_search, validate_token};

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
}
