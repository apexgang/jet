use jet_client::ClientError;
use jet_protocol::{ErrorCategory, FrameError, WireError};
use serde::Serialize;

/// Stable, bounded failure information that is safe to render in the webview.
/// Native error strings and protocol payloads stay in the Rust process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PublicError {
    pub(crate) category: &'static str,
    pub(crate) code: String,
    pub(crate) message: &'static str,
    pub(crate) retryable: bool,
}

impl PublicError {
    pub(crate) fn from_client(error: &ClientError) -> Self {
        match error {
            ClientError::Rejected(error)
            | ClientError::Remote(error)
            | ClientError::Disconnected(error) => Self::from_wire(error),
            ClientError::Incompatible { .. } => Self {
                category: "incompatible",
                code: "protocol.incompatible".into(),
                message: "This Plane uses an incompatible Jet protocol.",
                retryable: false,
            },
            ClientError::FeatureUnavailable { .. } => Self {
                category: "incompatible",
                code: "protocol.feature_unavailable".into(),
                message: "This feature is unavailable on the connected Plane.",
                retryable: false,
            },
            ClientError::Io(_)
            | ClientError::Frame(FrameError::Io(_) | FrameError::Closed)
            | ClientError::Closed => Self::offline(),
            ClientError::Frame(_) | ClientError::Control(_) | ClientError::Unexpected(_) => {
                Self::invalid_response()
            }
        }
    }

    pub(crate) fn invalid_event_order() -> Self {
        Self {
            category: "invalid_response",
            code: "protocol.invalid_event_order".into(),
            message: "The Plane returned events out of order.",
            retryable: false,
        }
    }

    fn invalid_response() -> Self {
        Self {
            category: "invalid_response",
            code: "protocol.invalid_response".into(),
            message: "The Plane returned an invalid response.",
            retryable: false,
        }
    }

    fn offline() -> Self {
        Self {
            category: "offline",
            code: "transport.offline".into(),
            message: "Jet could not reach this Plane.",
            retryable: true,
        }
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
        }
    }
}

fn safe_code(value: &str) -> Option<String> {
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
    use super::{safe_code, PublicError};
    use jet_client::ClientError;
    use jet_protocol::FrameError;
    use std::io;

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
