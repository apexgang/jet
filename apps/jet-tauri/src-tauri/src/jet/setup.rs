use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use jet_protocol::{
    AccountBindingList, CapabilityObservation, CapabilitySnapshot, CredentialState,
    CredentialStoreStatus, DegradedCondition, ExternalTool, PairingGate, PairingSnapshot, Project,
    ProjectDisposal, ProjectList, ProjectRemovalBinding, ProjectRemovalPreview, Registrability,
    RemovalObstacle, ToolAvailability, Worktree,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use super::{agents::KnownHarness, errors::PublicError, JetBridge};

const PREVIEW_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub(crate) struct SetupState {
    project_grants: Mutex<HashMap<Uuid, ProjectGrant>>,
    removal_grants: Mutex<HashMap<Uuid, RemovalGrant>>,
    account_commands: Mutex<HashMap<String, Uuid>>,
}

struct ProjectGrant {
    root: String,
    command_id: Uuid,
    created_at: Instant,
}

struct RemovalGrant {
    binding: ProjectRemovalBinding,
    permanent_warning: String,
    trash_command_id: Uuid,
    permanent_command_id: Uuid,
    created_at: Instant,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetupSnapshot {
    plane: PlaneSetupView,
    capabilities: CapabilityView,
    projects: Vec<ProjectView>,
    accounts: Vec<AccountView>,
    pairing: PairingView,
    issues: Vec<SetupIssueView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupIssueView {
    section: &'static str,
    error: PublicError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaneSetupView {
    core_version: String,
    daemon_starts: String,
    platform: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityView {
    harnesses: Vec<String>,
    crafts: Vec<CraftView>,
    credential_store: &'static str,
    credential_store_label: &'static str,
    degraded: Vec<String>,
    auth_providers: Vec<AuthProviderView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CraftView {
    id: String,
    version: String,
    harnesses: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthProviderView {
    pub(crate) provider: &'static str,
    pub(crate) harness: &'static str,
    pub(crate) label: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectView {
    id: String,
    name: String,
    root: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountView {
    id: String,
    label: String,
    provider: String,
    state: &'static str,
    state_label: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingView {
    gate: &'static str,
    paired_clients: usize,
    offer_pending: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectPreviewView {
    preview_id: Option<String>,
    root: String,
    verdict: &'static str,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRemovalPreviewView {
    preview_id: String,
    project_id: String,
    name: String,
    root: String,
    disk_use_bytes: String,
    live_runs: String,
    schedules: String,
    dirty_files: String,
    unpushed_commits: String,
    workspace_count: usize,
    obstacles: Vec<String>,
    permanent_warning: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MutationResult {
    id: String,
    name: String,
}

pub(crate) async fn load_setup(bridge: State<'_, JetBridge>) -> Result<SetupSnapshot, PublicError> {
    let client = bridge
        .local()
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let status = client
        .status()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge
        .planes
        .observe_status(super::planes::PlaneId::Local, &status);
    let mut issues = Vec::new();
    let capabilities = match client.capabilities(CapabilityObservation::Fresh).await {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(setup_issue("capabilities", &error));
            None
        }
    };
    let projects = match client.projects().await {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(setup_issue("projects", &error));
            None
        }
    };
    let accounts = match client
        .account_bindings(CapabilityObservation::LastObserved)
        .await
    {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(setup_issue("accounts", &error));
            None
        }
    };
    let pairing = match client.pairing().await {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(setup_issue("pairing", &error));
            None
        }
    };

    Ok(snapshot(
        status,
        capabilities,
        projects,
        accounts,
        pairing,
        issues,
    ))
}

pub(crate) async fn preview_project(
    bridge: State<'_, JetBridge>,
    path: String,
) -> Result<ProjectPreviewView, PublicError> {
    validate_absolute_path(&path)?;
    let client = bridge
        .local()
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let preview = client
        .preview_project(&path, CapabilityObservation::Fresh)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let (verdict, detail, registrable) = registrability_view(&preview.registrability);
    let preview_id = if registrable {
        let id = Uuid::new_v4();
        bridge
            .setup
            .project_grants
            .lock()
            .map_err(|_| PublicError::internal())?
            .insert(
                id,
                ProjectGrant {
                    root: preview.root.clone(),
                    command_id: Uuid::new_v4(),
                    created_at: Instant::now(),
                },
            );
        Some(id.to_string())
    } else {
        None
    };

    Ok(ProjectPreviewView {
        preview_id,
        root: safe_path(&preview.root),
        verdict,
        detail,
    })
}

pub(crate) async fn register_project(
    bridge: State<'_, JetBridge>,
    preview_id: String,
) -> Result<MutationResult, PublicError> {
    let preview_id = parse_id(&preview_id, "project.preview_invalid")?;
    let (root, command_id) = {
        let grants = bridge
            .setup
            .project_grants
            .lock()
            .map_err(|_| PublicError::internal())?;
        let grant = grants
            .get(&preview_id)
            .filter(|grant| grant.created_at.elapsed() <= PREVIEW_LIFETIME)
            .ok_or_else(|| expired_preview("project.preview_expired"))?;
        (grant.root.clone(), grant.command_id)
    };

    // ASVS 2.3.1 and 5.3.2: only a native-cached, Plane-canonical preview
    // can become a Path grant. The webview cannot replace the root.
    let project = bridge
        .local()
        .register_project(command_id, &root)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge
        .setup
        .project_grants
        .lock()
        .map_err(|_| PublicError::internal())?
        .remove(&preview_id);
    Ok(project_result(&project))
}

pub(crate) async fn preview_project_removal(
    bridge: State<'_, JetBridge>,
    project_id: String,
) -> Result<ProjectRemovalPreviewView, PublicError> {
    let project_id = parse_id(&project_id, "project.identifier_invalid")?;
    let client = bridge
        .local()
        .connect()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let preview = client
        .preview_project_removal(project_id)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let preview_id = Uuid::new_v4();
    bridge
        .setup
        .removal_grants
        .lock()
        .map_err(|_| PublicError::internal())?
        .insert(
            preview_id,
            RemovalGrant {
                binding: preview.binding.clone(),
                permanent_warning: preview.permanent_removal_warning.clone(),
                trash_command_id: Uuid::new_v4(),
                permanent_command_id: Uuid::new_v4(),
                created_at: Instant::now(),
            },
        );

    Ok(removal_view(preview_id, &preview))
}

pub(crate) async fn remove_project(
    bridge: State<'_, JetBridge>,
    preview_id: String,
    typed_name: String,
    permanent: bool,
) -> Result<MutationResult, PublicError> {
    validate_project_name(&typed_name)?;
    let preview_id = parse_id(&preview_id, "project.preview_invalid")?;
    let (binding, warning, command_id) = {
        let grants = bridge
            .setup
            .removal_grants
            .lock()
            .map_err(|_| PublicError::internal())?;
        let grant = grants
            .get(&preview_id)
            .filter(|grant| grant.created_at.elapsed() <= PREVIEW_LIFETIME)
            .ok_or_else(|| expired_preview("project.preview_expired"))?;
        (
            grant.binding.clone(),
            grant.permanent_warning.clone(),
            if permanent {
                grant.permanent_command_id
            } else {
                grant.trash_command_id
            },
        )
    };
    let disposal = if permanent {
        ProjectDisposal::Permanent {
            acknowledged_warning: warning,
        }
    } else {
        ProjectDisposal::SystemTrash
    };
    // ASVS 2.3.1, 8.3.1, and 15.3.3: the trusted native layer supplies
    // the exact server-issued binding and accepts only the two intended fields.
    let removed = bridge
        .local()
        .remove_project(command_id, binding, &typed_name, disposal)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge
        .setup
        .removal_grants
        .lock()
        .map_err(|_| PublicError::internal())?
        .remove(&preview_id);
    Ok(MutationResult {
        id: removed.project_id.to_string(),
        name: project_name(&removed.root),
    })
}

pub(crate) async fn bind_harness_account(
    bridge: State<'_, JetBridge>,
    provider: String,
) -> Result<MutationResult, PublicError> {
    let capabilities = bridge
        .local()
        .current_capabilities()
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    let option = auth_provider_options(&capabilities.harnesses)
        .into_iter()
        .find(|option| option.provider == provider)
        .ok_or_else(|| {
            PublicError::invalid_input(
                "account.provider_unavailable",
                "That Harness is not available on this Plane.",
            )
        })?;
    let command_id = {
        let mut commands = bridge
            .setup
            .account_commands
            .lock()
            .map_err(|_| PublicError::internal())?;
        *commands
            .entry(provider.clone())
            .or_insert_with(Uuid::new_v4)
    };

    // ASVS 13.3.1 and 14.3.3: this command carries only non-secret
    // metadata. Authentication stays with the Harness environment.
    let binding = bridge
        .local()
        .bind_harness_account(command_id, option.provider, option.label)
        .await
        .map_err(|error| PublicError::from_client(&error))?;
    bridge
        .setup
        .account_commands
        .lock()
        .map_err(|_| PublicError::internal())?
        .remove(&provider);
    Ok(MutationResult {
        id: binding.binding_id.to_string(),
        name: safe_text(&binding.label, 96, "Harness account"),
    })
}

fn snapshot(
    status: jet_protocol::PlaneStatus,
    capabilities: Option<CapabilitySnapshot>,
    projects: Option<ProjectList>,
    accounts: Option<AccountBindingList>,
    pairing: Option<PairingSnapshot>,
    issues: Vec<SetupIssueView>,
) -> SetupSnapshot {
    let (platform, capabilities) = match capabilities {
        Some(capabilities) => {
            let (credential_store, credential_store_label) = match capabilities.credential_store {
                CredentialStoreStatus::Available { .. } => ("available", "Secure storage ready"),
                CredentialStoreStatus::Locked { .. } => ("locked", "Secure storage locked"),
                CredentialStoreStatus::Unavailable { .. } => {
                    ("unavailable", "Secure storage unavailable")
                }
            };
            let platform = format!(
                "{} · {}",
                safe_text(
                    &capabilities.platform.operating_system,
                    32,
                    "Unknown system"
                ),
                safe_text(&capabilities.platform.architecture, 32, "Unknown CPU")
            );
            let auth_providers = auth_provider_options(&capabilities.harnesses);
            let view = CapabilityView {
                harnesses: capabilities
                    .harnesses
                    .iter()
                    .map(|value| safe_text(value, 64, "Unknown Harness"))
                    .collect(),
                crafts: capabilities
                    .crafts
                    .iter()
                    .map(|craft| CraftView {
                        id: safe_text(&craft.craft_id, 128, "unavailable"),
                        version: safe_text(&craft.version, 64, "Unknown"),
                        harnesses: craft
                            .harnesses
                            .iter()
                            .map(|value| safe_text(value, 64, "Unknown Harness"))
                            .collect(),
                    })
                    .collect(),
                credential_store,
                credential_store_label,
                degraded: capabilities.degraded.iter().map(degraded_label).collect(),
                auth_providers,
            };
            (platform, view)
        }
        None => (
            "Platform unavailable".into(),
            CapabilityView {
                harnesses: Vec::new(),
                crafts: Vec::new(),
                credential_store: "unavailable",
                credential_store_label: "Secure storage status unavailable",
                degraded: Vec::new(),
                auth_providers: Vec::new(),
            },
        ),
    };
    SetupSnapshot {
        plane: PlaneSetupView {
            core_version: safe_text(&status.core_version, 48, "Unknown"),
            daemon_starts: status.daemon_starts.to_string(),
            platform,
        },
        capabilities,
        projects: projects
            .map(|projects| projects.projects.iter().map(project_view).collect())
            .unwrap_or_default(),
        accounts: accounts
            .map(|accounts| {
                accounts
                    .bindings
                    .iter()
                    .map(|status| {
                        let (state, state_label) = credential_state(&status.credential_state);
                        AccountView {
                            id: status.binding.binding_id.to_string(),
                            label: safe_text(&status.binding.label, 96, "Harness account"),
                            provider: safe_text(&status.binding.provider, 64, "Unknown provider"),
                            state,
                            state_label,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        pairing: pairing
            .map(|pairing| PairingView {
                gate: match pairing.gate {
                    PairingGate::Open => "open",
                    PairingGate::Closed => "closed",
                },
                paired_clients: pairing.clients.len(),
                offer_pending: pairing.pending.is_some(),
            })
            .unwrap_or(PairingView {
                gate: "closed",
                paired_clients: 0,
                offer_pending: false,
            }),
        issues,
    }
}

fn setup_issue(section: &'static str, error: &jet_client::ClientError) -> SetupIssueView {
    SetupIssueView {
        section,
        error: PublicError::from_client(error),
    }
}

fn project_view(project: &Project) -> ProjectView {
    ProjectView {
        id: project.project_id.to_string(),
        name: project_name(&project.root),
        root: safe_path(&project.root),
    }
}

fn project_result(project: &Project) -> MutationResult {
    MutationResult {
        id: project.project_id.to_string(),
        name: project_name(&project.root),
    }
}

fn removal_view(id: Uuid, preview: &ProjectRemovalPreview) -> ProjectRemovalPreviewView {
    ProjectRemovalPreviewView {
        preview_id: id.to_string(),
        project_id: preview.binding.project_id.to_string(),
        name: project_name(&preview.binding.root),
        root: safe_path(&preview.binding.root),
        disk_use_bytes: preview.disk_use_bytes.to_string(),
        live_runs: preview.binding.live_runs.to_string(),
        schedules: preview.binding.schedules.to_string(),
        dirty_files: preview.binding.dirty_files.to_string(),
        unpushed_commits: preview.binding.unpushed_commits.to_string(),
        workspace_count: preview.binding.workspaces.len(),
        obstacles: preview
            .obstacles
            .iter()
            .map(|obstacle| obstacle_label(*obstacle).into())
            .collect(),
        permanent_warning: safe_text(
            &preview.permanent_removal_warning,
            512,
            "Permanent removal cannot be undone.",
        ),
    }
}

fn registrability_view(registrability: &Registrability) -> (&'static str, String, bool) {
    match registrability {
        Registrability::Registrable { repository } => {
            let worktree = match repository.worktree {
                Worktree::Main => "main working tree",
                Worktree::Linked { .. } => "linked working tree",
            };
            let lfs = match repository.lfs {
                ToolAvailability::Present { .. } => "Git LFS available",
                ToolAvailability::Missing => "Git LFS not installed",
            };
            ("registrable", format!("{worktree}, {lfs}"), true)
        }
        Registrability::NotARepository => (
            "not_a_repository",
            "Choose the top folder of a Git working tree.".into(),
            false,
        ),
        Registrability::BrokenRepository => (
            "broken_repository",
            "Git cannot open this repository.".into(),
            false,
        ),
        Registrability::BareRepository => (
            "bare_repository",
            "Jet needs a working tree, not a bare repository.".into(),
            false,
        ),
        Registrability::InsideGitDir => (
            "inside_git_directory",
            "Choose the working tree instead of its .git directory.".into(),
            false,
        ),
        Registrability::InsideWorkingTree { toplevel } => (
            "inside_working_tree",
            format!("Choose the repository root at {}.", safe_path(toplevel)),
            false,
        ),
    }
}

/// Harness-native sign-in options for the Plane's first-party Harnesses,
/// one per provider.
pub(crate) fn auth_provider_options(harnesses: &[String]) -> Vec<AuthProviderView> {
    let mut providers = Vec::new();
    let mut seen = HashSet::new();
    for harness in harnesses.iter().filter_map(|id| KnownHarness::of(id)) {
        if seen.insert(harness.provider()) {
            providers.push(AuthProviderView {
                provider: harness.provider(),
                harness: harness.name(),
                label: harness.login_label(),
            });
        }
    }
    providers
}

pub(crate) fn credential_state(state: &CredentialState) -> (&'static str, &'static str) {
    match state {
        CredentialState::Resolvable => ("ready", "Ready"),
        CredentialState::ResolvedAtUse => ("resolved_at_use", "Checked when used"),
        CredentialState::WaitingForUnlock { .. } => ("locked", "Unlock required"),
        CredentialState::Unavailable { .. } => ("unavailable", "Unavailable"),
        CredentialState::InvalidatedByRestart => ("invalidated", "Reconnect required"),
    }
}

pub(crate) fn degraded_label(condition: &DegradedCondition) -> String {
    match condition {
        DegradedCondition::MissingExternalTool { tool } => match tool {
            ExternalTool::Git => "Git is not available".into(),
            ExternalTool::GitLfs => "Git LFS is not available".into(),
            ExternalTool::Ssh => "SSH is not available".into(),
            ExternalTool::Tailscale => "Tailscale is not available".into(),
        },
        DegradedCondition::NoHarnessAvailable => "No Harness is installed".into(),
        DegradedCondition::CredentialStoreUnavailable { .. } => {
            "Secure storage is unavailable".into()
        }
        DegradedCondition::CredentialStoreLocked { .. } => "Secure storage is locked".into(),
    }
}

fn obstacle_label(obstacle: RemovalObstacle) -> &'static str {
    match obstacle {
        RemovalObstacle::LiveRuns => "Stop active Runs first",
        RemovalObstacle::Schedules => "Disable scheduled tasks first",
        RemovalObstacle::FilesystemRoot => "A filesystem root cannot be removed",
        RemovalObstacle::UserHome => "Your home directory cannot be removed",
        RemovalObstacle::JetHome => "Jet's data directory cannot be removed",
        RemovalObstacle::ContainsProject => "Remove nested Projects first",
    }
}

fn validate_absolute_path(path: &str) -> Result<(), PublicError> {
    // ASVS 2.2.1 and 5.3.2: this is usability validation only. The Plane
    // canonicalizes and authorizes the Path grant at the trusted boundary.
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || !Path::new(path).is_absolute()
    {
        return Err(PublicError::invalid_input(
            "project.path_invalid",
            "Enter an absolute folder path without control characters.",
        ));
    }
    Ok(())
}

fn validate_project_name(name: &str) -> Result<(), PublicError> {
    if name.is_empty()
        || name.len() > 255
        || name.chars().any(char::is_control)
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(PublicError::invalid_input(
            "project.name_invalid",
            "Type the Project folder name exactly as shown.",
        ));
    }
    Ok(())
}

fn parse_id(value: &str, code: &'static str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| {
        PublicError::invalid_input(code, "Refresh this view and try the action again.")
    })
}

fn expired_preview(code: &'static str) -> PublicError {
    PublicError::invalid_input(code, "This preview expired. Refresh it before continuing.")
}

pub(crate) fn project_name(root: &str) -> String {
    Path::new(root)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| safe_text(name, 255, "Project"))
        .unwrap_or_else(|| "Project".into())
}

fn safe_path(value: &str) -> String {
    safe_text(value, 4096, "Path unavailable")
}

fn safe_text(value: &str, maximum_bytes: usize, fallback: &str) -> String {
    if !value.is_empty()
        && value.len() <= maximum_bytes
        && value.chars().all(|character| !character.is_control())
    {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use jet_protocol::{PlaneStatus, ProjectList};
    use uuid::Uuid;

    use super::{
        auth_provider_options, snapshot, validate_absolute_path, validate_project_name,
        SetupIssueView,
    };
    use crate::jet::errors::PublicError;

    #[test]
    fn auth_options_come_only_from_supported_first_party_harnesses() {
        let options =
            auth_provider_options(&["codex".into(), "claude-code".into(), "unknown".into()]);
        assert_eq!(
            options
                .iter()
                .map(|option| option.provider)
                .collect::<Vec<_>>(),
            ["openai", "anthropic"]
        );
    }

    #[test]
    fn webview_paths_and_typed_names_are_narrowly_validated() {
        assert!(validate_absolute_path("/Users/example/project").is_ok());
        assert!(validate_absolute_path("relative/project").is_err());
        assert!(validate_absolute_path("/tmp/project\0secret").is_err());
        assert!(validate_project_name("project").is_ok());
        assert!(validate_project_name("../project").is_err());
    }

    #[test]
    fn setup_snapshot_preserves_available_sections_when_another_query_fails() {
        let snapshot = snapshot(
            PlaneStatus {
                cursor: Some(9),
                plane_id: Uuid::new_v4(),
                daemon_starts: 2,
                started_at_unix_ms: 1,
                core_version: "0.2.0".into(),
                security: None,
                recovery: None,
            },
            None,
            Some(ProjectList {
                cursor: 9,
                projects: Vec::new(),
            }),
            None,
            None,
            vec![SetupIssueView {
                section: "capabilities",
                error: PublicError::internal(),
            }],
        );

        assert_eq!(snapshot.plane.core_version, "0.2.0");
        assert_eq!(snapshot.plane.platform, "Platform unavailable");
        assert!(snapshot.projects.is_empty());
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].section, "capabilities");
    }
}
