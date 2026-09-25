//! Stable `service.*` codes of the local service (Wave 4 §A). The webview
//! keys its copy on the code; no native detail crosses with them.
use crate::jet::errors::PublicError;

/// A daemon from another installation channel owns the Plane (ADR-0026).
pub(crate) fn channel_owned() -> PublicError {
    PublicError::conflict(
        "service.channel_owned",
        "Another installation manages the Jet service on this computer.",
    )
}

/// `jetd core` exit 4: the owner did not relinquish the Plane in time.
pub(crate) fn drain_timeout() -> PublicError {
    PublicError::unavailable(
        "service.drain_timeout",
        "The Jet service didn't stop in time, so its version wasn't changed.",
        true,
    )
}

/// Staging, activating or registering the GUI-managed service failed.
pub(crate) fn install_failed() -> PublicError {
    PublicError::local_unavailable(
        "service.install_failed",
        "The Jet service couldn't be set up on this computer.",
        true,
    )
}

/// The system `tar` that unpacks the bundled payload was not found.
pub(crate) fn tar_missing() -> PublicError {
    PublicError::local_unavailable(
        "service.tar_missing",
        "Jet needs the tar program to set up its service. Install tar, then try Repair.",
        true,
    )
}

/// The socket did not answer within the start deadline.
pub(crate) fn start_timeout() -> PublicError {
    PublicError::unavailable(
        "service.start_timeout",
        "The Jet service didn't start in time.",
        true,
    )
}

/// A detached `jetd` could not be started (autostart mode).
pub(crate) fn start_failed() -> PublicError {
    PublicError::local_unavailable(
        "service.start_failed",
        "The Jet service couldn't be started.",
        true,
    )
}

/// `systemctl --user` refused a reload, enable or start.
pub(crate) fn systemd_unavailable() -> PublicError {
    PublicError::unavailable(
        "service.systemd_unavailable",
        "The system service manager didn't accept the Jet service.",
        true,
    )
}

/// `brew services start` failed.
pub(crate) fn homebrew_start_failed() -> PublicError {
    PublicError::unavailable(
        "service.homebrew_start_failed",
        "Homebrew couldn't start the Jet service.",
        true,
    )
}

/// The bundled archive is missing parts, is for another version or
/// architecture, or could not be unpacked.
pub(crate) fn payload_invalid() -> PublicError {
    PublicError::invalid_response_code(
        "service.payload_invalid",
        "The Jet service included with this app is damaged. Reinstall Jet.",
    )
}

/// `jetd core status` failed or printed something unreadable.
pub(crate) fn status_failed() -> PublicError {
    PublicError::local_unavailable(
        "service.status_failed",
        "Jet couldn't check its service on this computer.",
        true,
    )
}

/// The lock is held but its metadata is unreadable; nothing is drained.
pub(crate) fn owner_unknown() -> PublicError {
    PublicError::conflict(
        "service.owner_unknown",
        "A Jet service that Jet can't identify is running on this computer.",
    )
}

/// The compatibility rule refused the switch (target, protocol, schema).
pub(crate) fn switch_refused() -> PublicError {
    PublicError::conflict(
        "service.update_refused",
        "This version of the Jet service can't replace the one on this computer.",
    )
}

pub(crate) fn rollback_unavailable() -> PublicError {
    PublicError::conflict(
        "service.rollback_unavailable",
        "There is no earlier version of the Jet service to go back to.",
    )
}

pub(crate) fn rollback_failed() -> PublicError {
    PublicError::local_unavailable(
        "service.rollback_failed",
        "The Jet service couldn't go back to the earlier version.",
        true,
    )
}

/// Another provisioning pass holds the in-process or cross-process lock.
pub(crate) fn busy() -> PublicError {
    PublicError::unavailable(
        "service.busy",
        "Jet is already working on its service. Try again in a moment.",
        true,
    )
}

/// `local-service.lock` could not be opened (for example, a directory in
/// its place).
pub(crate) fn lock_unavailable() -> PublicError {
    PublicError::local_unavailable(
        "service.lock_unavailable",
        "Jet couldn't coordinate its service with other Jet windows on this computer.",
        true,
    )
}

pub(crate) fn review_expired() -> PublicError {
    PublicError::invalid_input(
        "service.review_expired",
        "This review expired. Review going back again.",
    )
}

pub(crate) fn review_stale() -> PublicError {
    PublicError::conflict(
        "service.review_stale",
        "The Jet service changed since this review. Review going back again.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jet::errors::safe_code;

    #[test]
    fn service_codes_are_safe_and_categorized() {
        let cases: [(PublicError, &str, bool); 18] = [
            (channel_owned(), "conflict", false),
            (drain_timeout(), "unavailable", true),
            (install_failed(), "internal", true),
            (tar_missing(), "internal", true),
            (start_timeout(), "unavailable", true),
            (start_failed(), "internal", true),
            (systemd_unavailable(), "unavailable", true),
            (homebrew_start_failed(), "unavailable", true),
            (payload_invalid(), "invalid_response", false),
            (status_failed(), "internal", true),
            (owner_unknown(), "conflict", false),
            (switch_refused(), "conflict", false),
            (rollback_unavailable(), "conflict", false),
            (rollback_failed(), "internal", true),
            (busy(), "unavailable", true),
            (lock_unavailable(), "internal", true),
            (review_expired(), "invalid_input", false),
            (review_stale(), "conflict", false),
        ];
        for (error, category, retryable) in cases {
            assert!(error.code.starts_with("service."), "{}", error.code);
            assert_eq!(safe_code(&error.code).as_deref(), Some(error.code.as_str()));
            assert_eq!(
                (error.category, error.retryable),
                (category, retryable),
                "{}",
                error.code
            );
        }
    }
}
