//! The main window's shell presentation (wave 3.4 §4.4): the last
//! destination kind, the sidebar and work panel preferences, the work panel
//! tab and both column widths. Client-local UI layout only: no Jet
//! identifier or content, never sent to a Plane, and not part of
//! `JetBridge`.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{errors::PublicError, local_store};

const PRESENTATION_FILE: &str = "shell-presentation.json";
const MAX_PRESENTATION_BYTES: u64 = 1024;
const PRESENTATION_VERSION: u8 = 1;
/// Column ranges of the Swift client (`DesktopShellView.swift:17,29`).
const SIDEBAR_WIDTH: (u16, u16, u16) = (210, 244, 300);
const WORK_PANEL_WIDTH: (u16, u16, u16) = (280, 340, 440);

const READ_FAILED: &str = "presentation.read_failed";
const WRITE_FAILED: &str = "presentation.write_failed";

/// Destinations worth reopening. Search, Needs attention, Trash and anything
/// newer reopen as the task view.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RestorableDestination {
    Conversation,
    NewTask,
    Project,
    Schedules,
    Planes,
    #[serde(other)]
    Other,
}

impl RestorableDestination {
    fn normalised(self) -> Self {
        match self {
            Self::Other => Self::Conversation,
            known => known,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WorkPanelTab {
    Changes,
    Files,
    Terminal,
    Run,
    Delivery,
    #[serde(other)]
    Other,
}

impl WorkPanelTab {
    fn normalised(self) -> Self {
        match self {
            Self::Other => Self::Run,
            known => known,
        }
    }
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellPresentation {
    pub version: u8,
    pub destination: RestorableDestination,
    pub sidebar_presented: bool,
    /// The regular-window column preference; the compact overlay is never saved.
    pub work_panel_presented: bool,
    pub work_panel_tab: WorkPanelTab,
    pub sidebar_width: u16,
    pub work_panel_width: u16,
}

impl Default for ShellPresentation {
    fn default() -> Self {
        Self {
            version: PRESENTATION_VERSION,
            destination: RestorableDestination::Conversation,
            sidebar_presented: true,
            work_panel_presented: false,
            work_panel_tab: WorkPanelTab::Run,
            sidebar_width: SIDEBAR_WIDTH.1,
            work_panel_width: WORK_PANEL_WIDTH.1,
        }
    }
}

/// What the file holds, one field at a time: an invalid field falls back to
/// its default while its valid siblings are kept.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawPresentation {
    version: Option<serde_json::Value>,
    destination: Option<serde_json::Value>,
    sidebar_presented: Option<serde_json::Value>,
    work_panel_presented: Option<serde_json::Value>,
    work_panel_tab: Option<serde_json::Value>,
    sidebar_width: Option<serde_json::Value>,
    work_panel_width: Option<serde_json::Value>,
}

fn field<T: serde::de::DeserializeOwned>(value: Option<serde_json::Value>) -> Option<T> {
    value.and_then(|value| serde_json::from_value(value).ok())
}

fn clamped_width(value: Option<serde_json::Value>, range: (u16, u16, u16)) -> u16 {
    match value.as_ref().and_then(serde_json::Value::as_f64) {
        Some(width) if width.is_finite() => {
            width.round().clamp(f64::from(range.0), f64::from(range.2)) as u16
        }
        _ => range.1,
    }
}

/// The stored presentation, or `None` when the whole file must be reset:
/// not JSON, not an object, or a version other than 1.
fn decode(bytes: &[u8]) -> Option<ShellPresentation> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if !value.is_object() {
        return None;
    }
    let raw: RawPresentation = serde_json::from_value(value).ok()?;
    if field::<u8>(raw.version) != Some(PRESENTATION_VERSION) {
        return None;
    }
    let defaults = ShellPresentation::default();
    Some(ShellPresentation {
        version: PRESENTATION_VERSION,
        destination: field::<RestorableDestination>(raw.destination)
            .map_or(defaults.destination, RestorableDestination::normalised),
        sidebar_presented: field(raw.sidebar_presented).unwrap_or(defaults.sidebar_presented),
        work_panel_presented: field(raw.work_panel_presented)
            .unwrap_or(defaults.work_panel_presented),
        work_panel_tab: field::<WorkPanelTab>(raw.work_panel_tab)
            .map_or(defaults.work_panel_tab, WorkPanelTab::normalised),
        sidebar_width: clamped_width(raw.sidebar_width, SIDEBAR_WIDTH),
        work_panel_width: clamped_width(raw.work_panel_width, WORK_PANEL_WIDTH),
    })
}

/// The file's presentation and any read problem. A missing file is not one.
fn load(path: &Path) -> (ShellPresentation, Option<&'static str>) {
    match local_store::read_bounded(path, MAX_PRESENTATION_BYTES) {
        Ok(None) => (ShellPresentation::default(), None),
        Ok(Some(bytes)) => match decode(&bytes) {
            Some(presentation) => (presentation, None),
            None => (ShellPresentation::default(), Some(READ_FAILED)),
        },
        Err(_) => (ShellPresentation::default(), Some(READ_FAILED)),
    }
}

/// The webview's save request. Widths arrive as numbers and are checked
/// here, so a fractional or out-of-range width is a stable refusal.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShellPresentationInput {
    version: f64,
    destination: RestorableDestination,
    sidebar_presented: bool,
    work_panel_presented: bool,
    work_panel_tab: WorkPanelTab,
    sidebar_width: f64,
    work_panel_width: f64,
}

fn out_of_range() -> PublicError {
    PublicError::invalid_input(
        "presentation.out_of_range",
        "Jet couldn't save the window layout.",
    )
}

fn width(value: f64, range: (u16, u16, u16)) -> Result<u16, PublicError> {
    if value.fract() != 0.0 || value < f64::from(range.0) || value > f64::from(range.2) {
        return Err(out_of_range());
    }
    Ok(value as u16)
}

/// Validates a save request. Unknown destinations and tabs from a newer
/// webview are normalised; widths and the version must be exact.
fn parse(value: serde_json::Value) -> Result<ShellPresentation, PublicError> {
    let input: ShellPresentationInput = serde_json::from_value(value).map_err(|_| {
        PublicError::invalid_input(
            "presentation.invalid",
            "Jet couldn't save the window layout.",
        )
    })?;
    if input.version != f64::from(PRESENTATION_VERSION) {
        return Err(out_of_range());
    }
    Ok(ShellPresentation {
        version: PRESENTATION_VERSION,
        destination: input.destination.normalised(),
        sidebar_presented: input.sidebar_presented,
        work_panel_presented: input.work_panel_presented,
        work_panel_tab: input.work_panel_tab.normalised(),
        sidebar_width: width(input.sidebar_width, SIDEBAR_WIDTH)?,
        work_panel_width: width(input.work_panel_width, WORK_PANEL_WIDTH)?,
    })
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellPresentationView {
    presentation: ShellPresentation,
    /// The last load or save problem in this process, so the Settings
    /// window can show it without the main window's session.
    issue: Option<&'static str>,
}

struct Stored {
    presentation: ShellPresentation,
    issue: Option<&'static str>,
    /// Bumped by every save; only the newest save updates memory.
    generation: u64,
}

pub(crate) struct PresentationState {
    directory: PathBuf,
    stored: Mutex<Stored>,
    /// Orders file writes so the newest save is the one left on disk. The
    /// state mutex is never held across I/O.
    write: tokio::sync::Mutex<()>,
}

impl PresentationState {
    /// Reads the file once, at startup. Never fails.
    pub(crate) fn new(app_data_directory: &Path) -> Self {
        let (presentation, issue) = load(&app_data_directory.join(PRESENTATION_FILE));
        Self {
            directory: app_data_directory.to_path_buf(),
            stored: Mutex::new(Stored {
                presentation,
                issue,
                generation: 0,
            }),
            write: tokio::sync::Mutex::new(()),
        }
    }

    /// What `load_shell_presentation` would answer.
    #[cfg(test)]
    pub(crate) fn loaded(&self) -> ShellPresentationView {
        self.view().unwrap()
    }

    fn view(&self) -> Result<ShellPresentationView, PublicError> {
        let stored = self.stored.lock().map_err(|_| PublicError::internal())?;
        Ok(ShellPresentationView {
            presentation: stored.presentation.clone(),
            issue: stored.issue,
        })
    }

    async fn save(
        &self,
        presentation: ShellPresentation,
    ) -> Result<ShellPresentationView, PublicError> {
        let generation = {
            let mut stored = self.stored.lock().map_err(|_| PublicError::internal())?;
            stored.generation += 1;
            stored.generation
        };
        let bytes = serde_json::to_vec(&presentation).map_err(|_| PublicError::internal())?;
        let written = {
            let _write = self.write.lock().await;
            let directory = self.directory.clone();
            tokio::task::spawn_blocking(move || {
                local_store::write_private_atomically(&directory, PRESENTATION_FILE, &bytes)
            })
            .await
            .map_err(|_| PublicError::internal())?
        };
        let mut stored = self.stored.lock().map_err(|_| PublicError::internal())?;
        if written.is_err() {
            stored.issue = Some(WRITE_FAILED);
            return Err(PublicError::local_unavailable(
                WRITE_FAILED,
                "Jet couldn't save the window layout.",
                true,
            ));
        }
        if generation == stored.generation {
            stored.presentation = presentation.clone();
        }
        if stored.issue == Some(WRITE_FAILED) {
            stored.issue = None;
        }
        Ok(ShellPresentationView {
            presentation,
            issue: stored.issue,
        })
    }
}

/// Served from memory; only takes a lock.
#[tauri::command]
pub(crate) fn load_shell_presentation(
    state: State<'_, PresentationState>,
) -> Result<ShellPresentationView, PublicError> {
    state.view()
}

#[tauri::command]
pub(crate) async fn save_shell_presentation(
    state: State<'_, PresentationState>,
    presentation: serde_json::Value,
) -> Result<ShellPresentationView, PublicError> {
    let presentation = parse(presentation)?;
    state.save(presentation).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn input(overrides: serde_json::Value) -> serde_json::Value {
        let mut value = json!({
            "version": 1,
            "destination": "project",
            "sidebarPresented": false,
            "workPanelPresented": true,
            "workPanelTab": "files",
            "sidebarWidth": 260,
            "workPanelWidth": 400
        });
        for (key, field) in overrides.as_object().unwrap() {
            value[key] = field.clone();
        }
        value
    }

    fn write_file(directory: &Path, contents: &str) {
        fs::write(directory.join(PRESENTATION_FILE), contents).unwrap();
    }

    #[test]
    fn default_matches_swift_ranges() {
        let presentation = ShellPresentation::default();
        assert_eq!(presentation.sidebar_width, 244);
        assert_eq!(presentation.work_panel_width, 340);
        assert_eq!(
            presentation.destination,
            RestorableDestination::Conversation
        );
        assert_eq!(presentation.work_panel_tab, WorkPanelTab::Run);
        assert!(presentation.sidebar_presented && !presentation.work_panel_presented);
        assert_eq!(SIDEBAR_WIDTH, (210, 244, 300));
        assert_eq!(WORK_PANEL_WIDTH, (280, 340, 440));
    }

    #[test]
    fn load_keeps_valid_fields_when_one_field_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        write_file(
            directory.path(),
            r#"{"version":1,"destination":"trash","workPanelTab":"changes","sidebarPresented":false,
               "workPanelPresented":"yes","sidebarWidth":250,"workPanelWidth":300,"future":1}"#,
        );
        let state = PresentationState::new(directory.path());
        let view = state.view().unwrap();
        assert_eq!(view.issue, None);
        assert_eq!(
            view.presentation,
            ShellPresentation {
                version: 1,
                destination: RestorableDestination::Conversation,
                sidebar_presented: false,
                work_panel_presented: false,
                work_panel_tab: WorkPanelTab::Changes,
                sidebar_width: 250,
                work_panel_width: 300,
            }
        );
    }

    #[test]
    fn load_resets_on_oversized_non_json_or_wrong_version_and_reports_read_failed() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            PresentationState::new(directory.path())
                .view()
                .unwrap()
                .issue,
            None,
            "a missing file is not a problem"
        );
        for contents in [
            "not json".to_owned(),
            "[1,2]".to_owned(),
            r#"{"version":2,"destination":"planes"}"#.to_owned(),
            r#"{"destination":"planes"}"#.to_owned(),
            format!(r#"{{"version":1,"pad":"{}"}}"#, " ".repeat(1100)),
        ] {
            write_file(directory.path(), &contents);
            let view = PresentationState::new(directory.path()).view().unwrap();
            assert_eq!(view.issue, Some(READ_FAILED), "{contents}");
            assert_eq!(view.presentation, ShellPresentation::default());
        }
    }

    #[test]
    fn load_clamps_out_of_range_widths() {
        let directory = tempfile::tempdir().unwrap();
        write_file(
            directory.path(),
            r#"{"version":1,"sidebarWidth":900,"workPanelWidth":12.4}"#,
        );
        let view = PresentationState::new(directory.path()).view().unwrap();
        assert_eq!(view.presentation.sidebar_width, 300);
        assert_eq!(view.presentation.work_panel_width, 280);
        write_file(
            directory.path(),
            r#"{"version":1,"sidebarWidth":"wide","workPanelWidth":-5}"#,
        );
        let view = PresentationState::new(directory.path()).view().unwrap();
        assert_eq!(view.presentation.sidebar_width, 244);
        assert_eq!(view.presentation.work_panel_width, 280);
    }

    #[test]
    fn save_normalises_unknown_destination_to_conversation() {
        let parsed = parse(input(
            json!({"destination": "search", "workPanelTab": "logs"}),
        ))
        .unwrap();
        assert_eq!(parsed.destination, RestorableDestination::Conversation);
        assert_eq!(parsed.work_panel_tab, WorkPanelTab::Run);
        let kept = parse(input(json!({}))).unwrap();
        assert_eq!(kept.destination, RestorableDestination::Project);
        assert_eq!(kept.work_panel_tab, WorkPanelTab::Files);
    }

    #[test]
    fn save_rejects_out_of_range_widths() {
        for overrides in [
            json!({"sidebarWidth": 209}),
            json!({"sidebarWidth": 301}),
            json!({"workPanelWidth": 279}),
            json!({"workPanelWidth": 441}),
            json!({"workPanelWidth": 300.5}),
            json!({"version": 2}),
        ] {
            assert_eq!(
                parse(input(overrides.clone())).unwrap_err().code,
                "presentation.out_of_range",
                "{overrides}"
            );
        }
        for overrides in [json!({"sidebarWidth": "wide"}), json!({"extra": true})] {
            assert_eq!(
                parse(input(overrides)).unwrap_err().code,
                "presentation.invalid"
            );
        }
    }

    #[tokio::test]
    async fn save_round_trips_and_is_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let state = PresentationState::new(directory.path());
        let saved = state.save(parse(input(json!({}))).unwrap()).await.unwrap();
        assert_eq!(saved.issue, None);
        assert_eq!(state.view().unwrap(), saved);
        assert_eq!(
            PresentationState::new(directory.path()).view().unwrap(),
            saved
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.path().join(PRESENTATION_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn a_failed_save_keeps_memory_and_reports_until_a_save_succeeds() {
        let directory = tempfile::tempdir().unwrap();
        let state = PresentationState {
            directory: directory.path().join("missing"),
            ..PresentationState::new(directory.path())
        };
        let error = state
            .save(parse(input(json!({}))).unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, WRITE_FAILED);
        assert!(error.retryable);
        let view = state.view().unwrap();
        assert_eq!(view.issue, Some(WRITE_FAILED));
        assert_eq!(view.presentation, ShellPresentation::default());

        fs::create_dir(directory.path().join("missing")).unwrap();
        let saved = state.save(parse(input(json!({}))).unwrap()).await.unwrap();
        assert_eq!(saved.issue, None);
    }

    #[tokio::test]
    async fn a_successful_save_keeps_a_read_problem_visible() {
        let directory = tempfile::tempdir().unwrap();
        write_file(directory.path(), "{");
        let state = PresentationState::new(directory.path());
        let saved = state.save(parse(input(json!({}))).unwrap()).await.unwrap();
        assert_eq!(saved.issue, Some(READ_FAILED));
    }

    #[test]
    fn serde_field_names_are_camel_case() {
        let view = ShellPresentationView {
            presentation: ShellPresentation {
                destination: RestorableDestination::NewTask,
                ..ShellPresentation::default()
            },
            issue: Some(READ_FAILED),
        };
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            json!({
                "presentation": {
                    "version": 1,
                    "destination": "new-task",
                    "sidebarPresented": true,
                    "workPanelPresented": false,
                    "workPanelTab": "run",
                    "sidebarWidth": 244,
                    "workPanelWidth": 340
                },
                "issue": "presentation.read_failed"
            })
        );
    }

    /// The literal arrays in `presentation.ts`, in order.
    fn typescript_literals(name: &str) -> Vec<String> {
        let source = include_str!("../../../src/lib/jet/presentation.ts");
        let start = source
            .find(&format!("export const {name} = ["))
            .unwrap_or_else(|| panic!("{name} is missing from presentation.ts"));
        let list = &source[start..];
        let list = &list[list.find('[').unwrap() + 1..list.find(']').unwrap()];
        list.split(',')
            .map(|literal| literal.trim().trim_matches('"').to_owned())
            .filter(|literal| !literal.is_empty())
            .collect()
    }

    fn rust_literals<T: Serialize>(values: &[T]) -> Vec<String> {
        values
            .iter()
            .map(|value| {
                serde_json::to_value(value)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn restorable_literals_match_typescript() {
        assert_eq!(
            typescript_literals("RESTORABLE_DESTINATIONS"),
            rust_literals(&[
                RestorableDestination::Conversation,
                RestorableDestination::NewTask,
                RestorableDestination::Project,
                RestorableDestination::Schedules,
                RestorableDestination::Planes,
            ])
        );
        assert_eq!(
            typescript_literals("WORK_PANEL_TABS"),
            rust_literals(&[
                WorkPanelTab::Changes,
                WorkPanelTab::Files,
                WorkPanelTab::Terminal,
                WorkPanelTab::Run,
                WorkPanelTab::Delivery,
            ])
        );
    }
}
