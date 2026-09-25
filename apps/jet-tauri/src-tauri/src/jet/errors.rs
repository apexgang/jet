use jet_client::ClientError;
use jet_protocol::{
    ConflictState, ErrorCategory, FrameError, RecoveryAction, RestartMetadata, RunLifecycle,
    WireError,
};
use serde::Serialize;

use super::deadline::{expired_wait, Wait};

/// Stable, bounded failure information that is safe to render in the webview.
/// Native error strings and protocol payloads stay in the Rust process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicError {
    pub(crate) category: &'static str,
    pub(crate) code: String,
    pub(crate) message: &'static str,
    pub(crate) retryable: bool,
    pub(crate) recovery_actions: Box<[PublicRecoveryAction]>,
    pub(crate) restart: Option<Box<PublicRestart>>,
    pub(crate) revision_conflict: Option<Box<PublicRevisionConflict>>,
    /// Set only from `ClientError::FeatureUnavailable`: the minor a request
    /// needed and the minor the connection negotiated.
    pub(crate) protocol_limit: Option<Box<ProtocolLimit>>,
    /// The opaque native Plane handle of the command that failed, so the
    /// webview binds recovery actions to that Plane only.
    pub(crate) plane_id: Option<Box<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProtocolLimit {
    pub(crate) required_minor: u32,
    pub(crate) negotiated_minor: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum PublicRecoveryAction {
    RefreshFile,
    RefreshConversation,
    RefreshRun,
    ResumeEvents { after: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "reason",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum PublicRestart {
    CursorExpired {
        minimum_available_cursor: String,
        current_snapshot_revision: String,
    },
    CursorAhead {
        current_snapshot_revision: String,
    },
    PaginationStale {
        current_snapshot_revision: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicRevisionConflict {
    current_revision: String,
    safe_state: PublicSafeState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum PublicSafeState {
    Conversation {
        conversation_id: String,
        revision: Option<String>,
    },
    Run {
        run_id: String,
        conversation_id: String,
        revision: String,
        lifecycle: &'static str,
    },
}

/// Anything that can fail a Plane request: a `jet_client` failure, or a
/// failure the shell classified itself while reaching a remote Plane.
pub(crate) trait ToPublic {
    fn to_public(&self) -> PublicError;
}

impl ToPublic for ClientError {
    fn to_public(&self) -> PublicError {
        PublicError::from_client_error(self)
    }
}

impl<T: ToPublic + ?Sized> ToPublic for Box<T> {
    fn to_public(&self) -> PublicError {
        (**self).to_public()
    }
}

impl ToPublic for PublicError {
    fn to_public(&self) -> PublicError {
        self.clone()
    }
}

impl PublicError {
    pub(crate) fn from_client<E: ToPublic + ?Sized>(error: &E) -> Self {
        error.to_public()
    }

    fn from_client_error(error: &ClientError) -> Self {
        match error {
            ClientError::Rejected(error)
            | ClientError::Remote(error)
            | ClientError::Disconnected(error) => Self::from_wire(error),
            ClientError::Incompatible { .. } => Self::new(
                "incompatible",
                "protocol.incompatible",
                "This Plane uses an incompatible Jet protocol.",
                false,
            ),
            ClientError::FeatureUnavailable {
                required_minor,
                negotiated_minor,
            } => Self {
                protocol_limit: Some(Box::new(ProtocolLimit {
                    required_minor: *required_minor,
                    negotiated_minor: *negotiated_minor,
                })),
                ..Self::new(
                    "incompatible",
                    "protocol.feature_unavailable",
                    "This feature is unavailable on the connected Plane.",
                    false,
                )
            },
            ClientError::Io(_) if expired_wait(error) == Some(Wait::Command) => {
                Self::command_outcome_unknown()
            }
            ClientError::Io(_)
            | ClientError::Frame(FrameError::Io(_) | FrameError::Closed)
            | ClientError::Closed => Self::offline(),
            ClientError::Frame(_) | ClientError::Control(_) | ClientError::Unexpected(_) => {
                Self::invalid_response()
            }
        }
    }

    fn new(
        category: &'static str,
        code: &'static str,
        message: &'static str,
        retryable: bool,
    ) -> Self {
        Self {
            category,
            code: code.into(),
            message,
            retryable,
            recovery_actions: Box::default(),
            restart: None,
            revision_conflict: None,
            protocol_limit: None,
            plane_id: None,
        }
    }

    pub(crate) fn invalid_event_order() -> Self {
        Self::new(
            "invalid_response",
            "protocol.invalid_event_order",
            "The Plane returned events out of order.",
            false,
        )
    }

    pub(crate) fn invalid_input(code: &'static str, message: &'static str) -> Self {
        Self::new("invalid_input", code, message, false)
    }

    /// A definite, non-retryable conflict with native or Plane state, such
    /// as `plane.review_moved`.
    pub(crate) fn conflict(code: &'static str, message: &'static str) -> Self {
        Self::new("conflict", code, message, false)
    }

    /// A definite, non-retryable authorization failure the shell concludes
    /// on its own, such as losing access after changing this computer's own
    /// Pairing on a remote Plane.
    pub(crate) fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self::new("unauthorized", code, message, false)
    }

    /// A Plane or local facility that cannot serve the request right now.
    pub(crate) fn unavailable(code: &'static str, message: &'static str, retryable: bool) -> Self {
        Self::new("unavailable", code, message, retryable)
    }

    /// A reply the shell refuses to trust, classified by call site.
    pub(crate) fn invalid_response_code(code: &'static str, message: &'static str) -> Self {
        Self::new("invalid_response", code, message, false)
    }

    /// A local facility of this app failed in a way a retry may fix, such
    /// as writing an exported file (`audit.export_failed`). Unlike
    /// `internal()`, the code names what failed.
    pub(crate) fn local_internal(code: &'static str, message: &'static str) -> Self {
        Self::new("internal", code, message, true)
    }

    /// A client-local facility of this app (a window, a local file) that
    /// could not do what was asked. The code names what failed, so the UI
    /// keys its copy on it; `retryable` says whether trying again may help.
    pub(crate) fn local_unavailable(
        code: &'static str,
        message: &'static str,
        retryable: bool,
    ) -> Self {
        Self::new("internal", code, message, retryable)
    }

    pub(crate) fn internal() -> Self {
        Self::new(
            "internal",
            "client.state_unavailable",
            "Jet could not complete the request.",
            true,
        )
    }

    /// Names the Plane a command resolved, unless an inner step already did.
    pub(crate) fn with_plane(mut self, plane_id: String) -> Self {
        if self.plane_id.is_none() {
            self.plane_id = Some(plane_id.into_boxed_str());
        }
        self
    }

    fn invalid_response() -> Self {
        Self::new(
            "invalid_response",
            "protocol.invalid_response",
            "The Plane returned an invalid response.",
            false,
        )
    }

    /// A Command whose answer did not arrive before its deadline
    /// (`deadline.rs`). It may have been applied, so the shell keeps its
    /// command ID and a retry resends the same request (ADR-0093).
    pub(crate) fn command_outcome_unknown() -> Self {
        Self::new(
            "outcome_unknown",
            "command.outcome_unknown",
            "The Plane didn't answer in time, so Jet can't tell whether this was done. Try again to resend the same request.",
            true,
        )
    }

    pub(crate) fn offline() -> Self {
        Self::new(
            "offline",
            "transport.offline",
            "Jet could not reach this Plane.",
            true,
        )
    }

    fn from_wire(error: &WireError) -> Self {
        // ASVS 2.2.1 and 8.2.1: branch on allowlisted structure and never
        // forward daemon/native text through this trust boundary.
        let category = category_name(error.category);
        Self {
            category,
            code: safe_code(&error.code).unwrap_or_else(|| format!("{category}.request_failed")),
            message: category_message(error.category),
            retryable: error.retryable,
            recovery_actions: error
                .recovery_actions
                .iter()
                .map(public_recovery_action)
                .collect(),
            restart: error.restart.map(public_restart).map(Box::new),
            revision_conflict: error.revision_conflict.as_ref().map(|conflict| {
                Box::new(PublicRevisionConflict {
                    current_revision: conflict.current_revision.to_string(),
                    safe_state: match &conflict.safe_state {
                        ConflictState::Conversation { conversation } => {
                            PublicSafeState::Conversation {
                                conversation_id: conversation.conversation_id.to_string(),
                                revision: conversation.revision.map(|value| value.to_string()),
                            }
                        }
                        ConflictState::Run { run } => PublicSafeState::Run {
                            run_id: run.run_id.to_string(),
                            conversation_id: run.conversation_id.to_string(),
                            revision: run.revision.to_string(),
                            lifecycle: lifecycle_name(run.lifecycle),
                        },
                    },
                })
            }),
            protocol_limit: None,
            plane_id: None,
        }
    }
}

fn public_recovery_action(action: &RecoveryAction) -> PublicRecoveryAction {
    match action {
        RecoveryAction::RefreshFile { .. } => PublicRecoveryAction::RefreshFile,
        RecoveryAction::RefreshConversation { .. } => PublicRecoveryAction::RefreshConversation,
        RecoveryAction::RefreshRun { .. } => PublicRecoveryAction::RefreshRun,
        RecoveryAction::ResumeEvents { after } => PublicRecoveryAction::ResumeEvents {
            after: after.to_string(),
        },
    }
}

fn public_restart(restart: RestartMetadata) -> PublicRestart {
    match restart {
        RestartMetadata::CursorExpired {
            minimum_available_cursor,
            current_snapshot_revision,
        } => PublicRestart::CursorExpired {
            minimum_available_cursor: minimum_available_cursor.to_string(),
            current_snapshot_revision: current_snapshot_revision.to_string(),
        },
        RestartMetadata::CursorAhead {
            current_snapshot_revision,
        } => PublicRestart::CursorAhead {
            current_snapshot_revision: current_snapshot_revision.to_string(),
        },
        RestartMetadata::PaginationStale {
            current_snapshot_revision,
        } => PublicRestart::PaginationStale {
            current_snapshot_revision: current_snapshot_revision.to_string(),
        },
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

/// A daemon code that may cross to the webview: dotted lowercase ASCII,
/// at most 96 bytes.
pub(crate) fn safe_code(value: &str) -> Option<String> {
    (value.len() <= 96
        && !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }))
    .then(|| value.to_owned())
}

fn category_name(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::InvalidInput => "invalid_input",
        ErrorCategory::Unauthorized => "unauthorized",
        ErrorCategory::Conflict => "conflict",
        ErrorCategory::Unavailable => "unavailable",
        ErrorCategory::Incompatible => "incompatible",
        ErrorCategory::RateLimited => "rate_limited",
        ErrorCategory::NotFound => "not_found",
        ErrorCategory::OutcomeUnknown => "outcome_unknown",
        ErrorCategory::Internal => "internal",
    }
}

fn category_message(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::InvalidInput => "Jet could not use that request.",
        ErrorCategory::Unauthorized => "This action is not authorized.",
        ErrorCategory::Conflict => "The Plane changed before the request completed.",
        ErrorCategory::Unavailable => "Jet is waiting for the local service.",
        ErrorCategory::Incompatible => "This Plane uses an incompatible Jet protocol.",
        ErrorCategory::RateLimited => "The Plane is temporarily limiting requests.",
        ErrorCategory::NotFound => "The requested Plane resource is no longer available.",
        ErrorCategory::OutcomeUnknown => "The request outcome could not be confirmed.",
        ErrorCategory::Internal => "The Plane could not complete the request.",
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_code, ProtocolLimit, PublicError};
    use jet_client::ClientError;
    use jet_protocol::{ErrorCategory, FrameError, WireError};
    use std::io;

    #[test]
    fn local_unavailable_keeps_code_and_retryable() {
        let error = PublicError::local_unavailable(
            "presentation.write_failed",
            "Jet couldn't save the window layout.",
            true,
        );
        assert_eq!(error.category, "internal");
        assert_eq!(error.code, "presentation.write_failed");
        assert!(error.retryable);
        let fixed = PublicError::local_unavailable(
            "window.mode_unavailable",
            "Full screen isn't available in this window.",
            false,
        );
        assert_eq!(fixed.code, "window.mode_unavailable");
        assert!(!fixed.retryable);
        assert!(fixed.recovery_actions.is_empty());
        assert_eq!(fixed.restart, None);
    }

    #[test]
    fn feature_unavailable_keeps_both_protocol_minors() {
        let error = PublicError::from_client(&ClientError::FeatureUnavailable {
            required_minor: 12,
            negotiated_minor: 9,
        });
        assert_eq!(error.code, "protocol.feature_unavailable");
        assert_eq!(
            error.protocol_limit.as_deref(),
            Some(&ProtocolLimit {
                required_minor: 12,
                negotiated_minor: 9,
            })
        );
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["protocolLimit"]["requiredMinor"], 12);
        assert_eq!(json["protocolLimit"]["negotiatedMinor"], 9);
        assert!(json["planeId"].is_null());
    }

    #[test]
    fn rejected_login_is_unauthorized_and_not_retryable() {
        let error = PublicError::from_client(&ClientError::Rejected(WireError {
            category: ErrorCategory::Unauthorized,
            code: "connection.unauthorized".into(),
            retryable: false,
            message: "daemon text never crosses".into(),
            revision_conflict: None,
            restart: None,
            recovery_actions: Vec::new(),
        }));
        assert_eq!(error.category, "unauthorized");
        assert_eq!(error.code, "connection.unauthorized");
        assert!(!error.retryable);
        assert_eq!(error.protocol_limit, None);
    }

    #[test]
    fn the_first_resolved_plane_names_the_error() {
        let error = PublicError::internal()
            .with_plane("local".into())
            .with_plane("00000000-0000-0000-0000-000000000002".into());
        assert_eq!(error.plane_id.as_deref(), Some("local"));
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["planeId"], "local");
    }

    /// Wave 3.3 local stable codes (§4.6): each passes the allowlist and
    /// keeps the category its constructor gives it.
    #[test]
    fn wave_3_3_local_codes_are_safe_and_categorized() {
        let invalid_input = [
            "retention.review_expired",
            "recovery.review_expired",
            "client.review_limit",
            "client.review_used",
            "client.review_plane_mismatch",
            "retention.names_invalid",
            "retention.stop_unacknowledged",
            "autodelete.rule_unknown",
            "autodelete.prompt_invalid",
            "autodelete.days_invalid",
            "audit.export_too_large",
        ];
        let conflict = [
            "retention.request_unresolved",
            "retention.mode_locked",
            "retention.review_stale",
            "retention.trash_unknown",
            "autodelete.retry_mismatch",
            "autodelete.token_expired",
            "recovery.snapshot_gone",
            "recovery.not_read_only_local",
            "recovery.purge_unavailable",
            "recovery.request_unresolved",
            "audit.export_required",
            "audit.export_busy",
            "storage.collect_busy",
        ];
        for code in invalid_input {
            assert_eq!(safe_code(code).as_deref(), Some(code));
            let error = PublicError::invalid_input(code, "m");
            assert_eq!((error.category, error.retryable), ("invalid_input", false));
        }
        for code in conflict {
            assert_eq!(safe_code(code).as_deref(), Some(code));
            let error = PublicError::conflict(code, "m");
            assert_eq!((error.category, error.retryable), ("conflict", false));
        }
        let failed = PublicError::local_internal("audit.export_failed", "m");
        assert_eq!(
            safe_code(&failed.code).as_deref(),
            Some("audit.export_failed")
        );
        assert_eq!((failed.category, failed.retryable), ("internal", true));
        // `internal()` keeps its own code.
        assert_eq!(PublicError::internal().code, "client.state_unavailable");
    }

    /// A Revision conflict crosses as its safe state only: the current
    /// revision as a decimal string and the identifiers the webview needs to
    /// replace its stale copy, never the daemon's text.
    #[test]
    fn a_revision_conflict_crosses_with_its_safe_state() {
        use jet_protocol::{
            ConflictState, Conversation, RetentionPolicy, RevisionConflict, Run, RunLifecycle,
        };
        use uuid::Uuid;

        let conflict = |safe_state| {
            PublicError::from_client(&ClientError::Remote(WireError {
                category: ErrorCategory::Conflict,
                code: "run.revision_conflict".into(),
                retryable: false,
                message: "daemon text never crosses".into(),
                revision_conflict: Some(RevisionConflict {
                    current_revision: 18_446_744_073_709_551_615,
                    safe_state,
                }),
                restart: None,
                recovery_actions: Vec::new(),
            }))
        };
        let run = conflict(ConflictState::Run {
            run: Run {
                run_id: Uuid::from_u128(0x70),
                conversation_id: Uuid::from_u128(0x71),
                revision: 7,
                lifecycle: RunLifecycle::Stopping,
                name: None,
                created_at_unix_ms: 1,
                ended_at_unix_ms: None,
            },
        });
        assert_eq!(
            (run.category, run.code.as_str()),
            ("conflict", "run.revision_conflict")
        );
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(
            json["revisionConflict"],
            serde_json::json!({
                "currentRevision": "18446744073709551615",
                "safeState": {
                    "type": "run",
                    "runId": "00000000-0000-0000-0000-000000000070",
                    "conversationId": "00000000-0000-0000-0000-000000000071",
                    "revision": "7",
                    "lifecycle": "stopping",
                },
            })
        );
        assert!(!json.to_string().contains("daemon text"));

        let conversation = conflict(ConflictState::Conversation {
            conversation: Conversation {
                conversation_id: Uuid::from_u128(0x71),
                revision: None,
                retention: RetentionPolicy::Retain,
                working_tree: None,
                origin: None,
                name: None,
                created_at_unix_ms: 1,
            },
        });
        assert_eq!(
            serde_json::to_value(&conversation).unwrap()["revisionConflict"]["safeState"],
            serde_json::json!({
                "type": "conversation",
                "conversationId": "00000000-0000-0000-0000-000000000071",
                "revision": null,
            })
        );
    }

    #[test]
    fn stable_error_codes_are_allowlisted_and_bounded() {
        assert_eq!(
            safe_code("transport.offline"),
            Some("transport.offline".into())
        );
        assert_eq!(safe_code("<script>"), None);
        assert_eq!(safe_code(&"a".repeat(97)), None);
    }

    #[test]
    fn transport_loss_uses_the_shared_offline_taxonomy() {
        let error = PublicError::from_client(&ClientError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "native detail must not cross IPC",
        )));

        assert_eq!(error.category, "offline");
        assert_eq!(error.code, "transport.offline");
        assert!(error.retryable);
    }

    #[test]
    fn malformed_frames_use_the_shared_invalid_response_taxonomy() {
        let error = PublicError::from_client(&ClientError::Frame(FrameError::UnknownKind(255)));

        assert_eq!(error.category, "invalid_response");
        assert_eq!(error.code, "protocol.invalid_response");
        assert!(!error.retryable);
    }
}
