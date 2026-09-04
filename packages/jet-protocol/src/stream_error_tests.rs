use pretty_assertions::assert_eq;

use super::StreamQueueError;
use crate::{ErrorCategory, RecoveryAction, WireError};

#[test]
fn only_slow_consumers_produce_a_stable_recovery_error() {
	assert_eq!(
		(
			StreamQueueError::SlowConsumer { resume_after: 41 }
				.disconnect_error(),
			StreamQueueError::EmptyData.disconnect_error(),
		),
		(
			Some(WireError {
				category: ErrorCategory::Unavailable,
				code: "protocol.slow_consumer".into(),
				retryable: true,
				message: "the Event consumer exceeded its bounded window; reconnect and replay after the supplied cursor".into(),
				revision_conflict: None,
				restart: None,
				recovery_actions: vec![RecoveryAction::ResumeEvents {
					after: 41,
				}],
			}),
			None,
		)
	);
}
