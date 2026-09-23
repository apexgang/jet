//! Per-Plane protocol knowledge. The shell cannot read the negotiated minor
//! (`jet-client` keeps `negotiated_minor()` crate-private), so it keeps the
//! bounds it can prove: optional status fields, `FeatureUnavailable` and
//! successful gated calls.
use jet_protocol::{
    PlaneStatus, APPROVAL_RETRY_MINOR, FENCED_READS_MINOR, GIT_DELIVERY_MINOR, PAIRING_MINOR,
    PROJECTS_MINOR, REMOTE_AUTH_MINOR, SEARCH_MINOR, SECURITY_AUDIT_MINOR,
    SETTINGS_AND_CAPABILITIES_MINOR, STORE_RECOVERY_MINOR, TURN_QUEUE_MINOR, WORKSPACES_MINOR,
    WORKSPACE_TERMINALS_MINOR,
};
use serde::Serialize;

/// Features shown in the Plane detail table, in display order. The minors
/// come from `jet-protocol`; they are never written as literals here.
pub(crate) const FEATURES: [(&str, u32); 11] = [
    ("remote_login", REMOTE_AUTH_MINOR),
    ("pairing", PAIRING_MINOR),
    ("capabilities", SETTINGS_AND_CAPABILITIES_MINOR),
    ("conversation_pages", FENCED_READS_MINOR),
    ("projects", PROJECTS_MINOR),
    ("workspaces", WORKSPACES_MINOR),
    ("search", SEARCH_MINOR),
    ("run_supervision", TURN_QUEUE_MINOR),
    ("workspace_terminals", WORKSPACE_TERMINALS_MINOR),
    ("approval_retry", APPROVAL_RETRY_MINOR),
    ("git_delivery", GIT_DELIVERY_MINOR),
];

/// What this client can prove about a Plane's negotiated minor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProtocolKnowledge {
    exact: Option<u32>,
    at_least: u32,
    at_most: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureView {
    feature: &'static str,
    required_minor: u32,
    support: &'static str,
}

/// The raw bounds, for the feature table's "this Plane: 1.{m}" copy only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProtocolView {
    exact: Option<u32>,
    at_least: u32,
    at_most: Option<u32>,
}

impl ProtocolKnowledge {
    /// Applies the presence rules of the daemon's status translator: `cursor`
    /// exists exactly from minor 1 and `recovery` exactly from minor 37.
    /// `security` is only a lower bound, because a read-only store whose audit
    /// could not be validated omits it at any minor.
    pub(crate) fn observe_status(&mut self, status: &PlaneStatus) {
        let mut observation = Self::default();
        if status.cursor.is_some() {
            observation.at_least = FENCED_READS_MINOR;
        } else {
            observation.at_most = Some(FENCED_READS_MINOR - 1);
        }
        if status.recovery.is_some() {
            observation.at_least = observation.at_least.max(STORE_RECOVERY_MINOR);
        } else {
            observation.at_most = Some(
                observation
                    .at_most
                    .map_or(STORE_RECOVERY_MINOR - 1, |value| {
                        value.min(STORE_RECOVERY_MINOR - 1)
                    }),
            );
        }
        if status.security.is_some() {
            observation.at_least = observation.at_least.max(SECURITY_AUDIT_MINOR);
        }
        if observation.consistent() {
            self.merge(observation);
        }
    }

    /// A gated call that succeeded proves the minor is at least `required`.
    pub(crate) fn observe_success(&mut self, required: u32) {
        self.merge(Self {
            exact: None,
            at_least: required,
            at_most: None,
        });
    }

    /// `FeatureUnavailable` names the negotiated minor exactly.
    pub(crate) fn observe_negotiated(&mut self, negotiated: u32) {
        self.merge(Self {
            exact: Some(negotiated),
            at_least: negotiated,
            at_most: Some(negotiated),
        });
    }

    /// Merges an observation. A contradiction means the Plane was upgraded or
    /// downgraded since the older facts, so the new observation replaces them.
    fn merge(&mut self, observation: Self) {
        let merged = Self {
            exact: observation.exact.or(self.exact),
            at_least: self.at_least.max(observation.at_least),
            at_most: match (self.at_most, observation.at_most) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            },
        };
        *self = if merged.consistent() {
            merged
        } else {
            observation
        };
    }

    fn consistent(&self) -> bool {
        self.at_most.is_none_or(|most| self.at_least <= most)
            && self.exact.is_none_or(|exact| {
                exact >= self.at_least && self.at_most.is_none_or(|most| exact <= most)
            })
    }

    pub(crate) fn support(&self, required: u32) -> &'static str {
        if let Some(exact) = self.exact {
            return if required <= exact {
                "supported"
            } else {
                "unsupported"
            };
        }
        if required <= self.at_least {
            "supported"
        } else if self.at_most.is_some_and(|most| required > most) {
            "unsupported"
        } else {
            "unknown"
        }
    }

    pub(crate) fn features(&self) -> Vec<FeatureView> {
        FEATURES
            .iter()
            .map(|(feature, required_minor)| FeatureView {
                feature,
                required_minor: *required_minor,
                support: self.support(*required_minor),
            })
            .collect()
    }

    pub(crate) fn view(&self) -> ProtocolView {
        ProtocolView {
            exact: self.exact,
            at_least: self.at_least,
            at_most: self.at_most,
        }
    }
}

#[cfg(test)]
mod tests {
    use jet_protocol::{
        PlaneStatus, RecoveryState, RecoveryStatus, SecurityState, SEARCH_MINOR,
        STORE_RECOVERY_MINOR,
    };
    use uuid::Uuid;

    use super::{ProtocolKnowledge, FEATURES};

    fn status(cursor: bool, security: bool, recovery: bool) -> PlaneStatus {
        PlaneStatus {
            cursor: cursor.then_some(4),
            plane_id: Uuid::from_u128(1),
            daemon_starts: 1,
            started_at_unix_ms: 1,
            core_version: "0.2.0".into(),
            security: security.then_some(SecurityState::Trusted),
            recovery: recovery.then(|| RecoveryStatus {
                state: RecoveryState::Serving,
                reason: None,
                snapshots: Vec::new(),
                deletion_ledger: None,
            }),
        }
    }

    #[test]
    fn every_feature_is_settled_by_the_recovery_presence_shortcut() {
        for (feature, required) in FEATURES {
            assert!(
                required <= STORE_RECOVERY_MINOR,
                "{feature} needs {required}, beyond the recovery-presence bound"
            );
        }
        let mut knowledge = ProtocolKnowledge::default();
        knowledge.observe_status(&status(true, true, true));
        assert!(knowledge
            .features()
            .iter()
            .all(|feature| feature.support == "supported"));
    }

    #[test]
    fn status_presence_rules_bound_the_minor() {
        let mut minor_zero = ProtocolKnowledge::default();
        minor_zero.observe_status(&status(false, false, false));
        assert_eq!(minor_zero.support(1), "unsupported");
        assert_eq!(minor_zero.view().at_most, Some(0));

        let mut older = ProtocolKnowledge::default();
        older.observe_status(&status(true, true, false));
        assert_eq!(older.support(5), "supported");
        assert_eq!(older.support(SEARCH_MINOR), "unknown");
        assert_eq!(older.support(STORE_RECOVERY_MINOR), "unsupported");
        assert_eq!(older.view().at_most, Some(STORE_RECOVERY_MINOR - 1));

        // A missing `security` field proves nothing about the minor.
        let mut read_only = ProtocolKnowledge::default();
        read_only.observe_status(&status(true, false, true));
        assert_eq!(read_only.support(5), "supported");
    }

    #[test]
    fn exact_success_and_contradiction_rules() {
        let mut knowledge = ProtocolKnowledge::default();
        knowledge.observe_status(&status(true, true, false));
        knowledge.observe_success(SEARCH_MINOR);
        assert_eq!(knowledge.support(SEARCH_MINOR), "supported");
        assert_eq!(knowledge.support(16), "unknown");

        knowledge.observe_negotiated(14);
        assert_eq!(knowledge.support(SEARCH_MINOR), "supported");
        assert_eq!(knowledge.support(16), "unsupported");
        assert_eq!(knowledge.view().exact, Some(14));

        // The Plane was upgraded: a status with `recovery` contradicts 14.
        knowledge.observe_status(&status(true, true, true));
        assert_eq!(knowledge.view().exact, None);
        assert_eq!(knowledge.support(35), "supported");

        // And downgraded again: an exact minor below the bound resets it.
        knowledge.observe_negotiated(9);
        assert_eq!(knowledge.view().exact, Some(9));
        assert_eq!(knowledge.support(SEARCH_MINOR), "unsupported");
        assert_eq!(knowledge.support(1), "supported");
    }
}
