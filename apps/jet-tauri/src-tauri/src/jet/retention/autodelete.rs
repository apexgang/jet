//! Auto-delete rules in the Settings window (Wave 3.3 §4.3): reading every
//! rule with its candidate matches, the drafting disclosure and the Utility
//! attribution, and changing one rule.
//!
//! The webview never names what it approves. A read issues an opaque token
//! for a draft's interpretation (Approve) and for an approved Forget rule
//! (delete everywhere); the token resolves natively to the rule and the
//! inactivity the user was shown, and stays the same while that binding is
//! unchanged, so a polling reload never invalidates an open dialog.
//!
//! Command IDs follow one pending body per rule slot (tauri-conventions
//! §1.6): the same change while unconfirmed resends its Command ID, and a
//! different change for that rule is refused until the first resolves.
use std::{collections::HashMap, sync::Mutex};

use jet_client::{Client, ClientError};
use jet_protocol::{
    AutodeleteCandidate, AutodeleteRule, AutodeleteRuleState, AutodeleteRules, AutodeleteScope,
    SettingKey, SettingValue, UtilityOutcome, UtilityPurpose, AUTODELETE_MINOR,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use super::{definite, plane_label, plane_setting, protection_view, ProtectionView};
use crate::jet::{
    errors::{safe_code, PublicError},
    planes::{PlaneBinding, PlaneId},
    JetBridge,
};

/// The Plane lists at most this many rules (`protocol autodelete.rs`).
const RULES_LIMIT: usize = 64;
/// Each rule lists at most this many candidates.
const CANDIDATES_LIMIT: usize = 32;
/// Prompt bound in UTF-8 bytes, as the Plane's (`utility.input_limit`).
const PROMPT_LIMIT: usize = 4096;
/// Whole days of inactivity a rule may name.
const DAYS_RANGE: std::ops::RangeInclusive<u32> = 1..=36_500;
/// Utility Queries one rules read may issue for attribution.
const ATTRIBUTION_LOOKUPS: usize = 64;
/// Attributions remembered across reads.
const ATTRIBUTION_CAPACITY: usize = 256;
/// Unconfirmed rule changes kept at once.
const PENDING_CAPACITY: usize = 256;
const PROVIDER_LIMIT: usize = 64;
const MODEL_LIMIT: usize = 96;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Which rule a change is for. A new rule has no ID until its first change
/// is sent, so all new-rule compiles share one slot per Plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RuleSlot {
    Rule(Uuid),
    NewRule,
}

/// A change exactly as it is sent. The approval carries the native
/// inactivity, never a webview value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RuleBody {
    Compile { prompt: String },
    SetDays { days: u32 },
    Approve { days: u32 },
    Everywhere,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRule {
    rule_id: Uuid,
    body: RuleBody,
    command_id: Uuid,
    /// The token an Approve or delete-everywhere change was made with, so
    /// "Try again" resolves even after the token left the rule list.
    token: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateKind {
    Compiling,
    Refused,
    Draft,
    Approved,
}

/// What a token was issued for. A token lives exactly as long as this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuleBinding {
    kind: StateKind,
    inactive_days: Option<u32>,
    updated_at_unix_ms: i64,
    scope: AutodeleteScope,
}

impl RuleBinding {
    fn of(rule: &AutodeleteRule) -> Self {
        let (kind, inactive_days) = match &rule.state {
            AutodeleteRuleState::Compiling => (StateKind::Compiling, None),
            AutodeleteRuleState::Refused { .. } => (StateKind::Refused, None),
            AutodeleteRuleState::Draft { inactive_days } => {
                (StateKind::Draft, Some(*inactive_days))
            }
            AutodeleteRuleState::Approved { inactive_days, .. } => {
                (StateKind::Approved, Some(*inactive_days))
            }
        };
        Self {
            kind,
            inactive_days,
            updated_at_unix_ms: rule.updated_at_unix_ms,
            scope: rule.scope,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleTokens {
    binding: RuleBinding,
    /// Issued for a draft: approves exactly its inactivity.
    approve: Option<Uuid>,
    /// Issued for an approved Forget rule: authorizes delete everywhere.
    everywhere: Option<Uuid>,
}

impl RuleTokens {
    fn issue(binding: RuleBinding) -> Self {
        Self {
            binding,
            approve: (binding.kind == StateKind::Draft).then(Uuid::new_v4),
            everywhere: (binding.kind == StateKind::Approved
                && binding.scope == AutodeleteScope::Forget)
                .then(Uuid::new_v4),
        }
    }
}

/// The rules of one Plane as last read, for the Plane identity read.
#[derive(Debug)]
struct PlaneRules {
    binding: PlaneBinding,
    rules: HashMap<Uuid, RuleTokens>,
}

/// A Utility job's attribution, cleaned natively. Kept only once the job
/// has answered: the Provider and Model it records never change after.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JobAttribution {
    provider: Option<String>,
    model: Option<String>,
    /// The inactivity the model drafted, if it drafted one.
    drafted_days: Option<u32>,
}

#[derive(Default)]
pub(crate) struct RuleState {
    pending: Mutex<HashMap<(PlaneBinding, RuleSlot), PendingRule>>,
    tokens: Mutex<HashMap<PlaneId, PlaneRules>>,
    attributions: Mutex<HashMap<(PlaneBinding, Uuid), JobAttribution>>,
}

/// A validated change, before it is bound to a rule slot.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedChange {
    Compile { rule: Option<Uuid>, prompt: String },
    SetDays { rule: Uuid, days: u32 },
    Approve { token: Uuid },
    Everywhere { token: Uuid },
    Delete { rule: Uuid },
}

/// A change bound to its slot and Command ID. `fresh` is false when the
/// same change was already sent and is being sent again.
#[derive(Debug, Clone)]
struct Claim {
    slot: RuleSlot,
    pending: PendingRule,
    fresh: bool,
}

fn token_expired() -> PublicError {
    PublicError::conflict(
        "autodelete.token_expired",
        "This rule changed. Reload to review its current version.",
    )
}

fn rule_unknown() -> PublicError {
    PublicError::invalid_input(
        "autodelete.rule_unknown",
        "Jet doesn't know this rule. Reload the rules.",
    )
}

impl RuleState {
    /// The Plane's store was replaced by a Recovery snapshot: approval
    /// tokens and unconfirmed changes read from the old store are dropped.
    pub(super) fn plane_restored(&self, plane: PlaneId) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(&plane);
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|(binding, _), _| binding.plane != plane);
        }
    }

    /// Binds a change to its rule slot: the Command ID of the same change
    /// still unconfirmed, or a new one when the slot is free.
    fn claim(&self, binding: PlaneBinding, change: ParsedChange) -> Result<Claim, PublicError> {
        let tokens = self.tokens.lock().map_err(|_| PublicError::internal())?;
        let plane_rules = tokens
            .get(&binding.plane)
            .filter(|rules| rules.binding == binding);
        let mut pending = self.pending.lock().map_err(|_| PublicError::internal())?;

        let (slot, body, token) = match change {
            ParsedChange::Compile { rule, prompt } => (
                rule.map_or(RuleSlot::NewRule, RuleSlot::Rule),
                RuleBody::Compile { prompt },
                None,
            ),
            ParsedChange::SetDays { rule, days } => {
                (RuleSlot::Rule(rule), RuleBody::SetDays { days }, None)
            }
            ParsedChange::Delete { rule } => (RuleSlot::Rule(rule), RuleBody::Delete, None),
            ParsedChange::Approve { token } | ParsedChange::Everywhere { token } => {
                // "Try again" of a change made with this token.
                if let Some(((_, slot), entry)) = pending
                    .iter()
                    .find(|((held, _), entry)| *held == binding && entry.token == Some(token))
                {
                    return Ok(Claim {
                        slot: *slot,
                        pending: entry.clone(),
                        fresh: false,
                    });
                }
                let approve = matches!(change, ParsedChange::Approve { .. });
                let (rule, tokens) = plane_rules
                    .and_then(|rules| {
                        rules.rules.iter().find(|(_, tokens)| {
                            if approve {
                                tokens.approve == Some(token)
                            } else {
                                tokens.everywhere == Some(token)
                            }
                        })
                    })
                    .ok_or_else(token_expired)?;
                let body = if approve {
                    RuleBody::Approve {
                        days: tokens
                            .binding
                            .inactive_days
                            .ok_or_else(PublicError::internal)?,
                    }
                } else {
                    RuleBody::Everywhere
                };
                (RuleSlot::Rule(*rule), body, Some(token))
            }
        };

        if let Some(entry) = pending.get(&(binding, slot)) {
            return if entry.body == body {
                Ok(Claim {
                    slot,
                    pending: entry.clone(),
                    fresh: false,
                })
            } else {
                Err(PublicError::conflict(
                    "autodelete.retry_mismatch",
                    "Jet is still confirming your previous change to this rule. Try that one again first.",
                ))
            };
        }
        let rule_id = match slot {
            RuleSlot::Rule(rule) => {
                if !plane_rules.is_some_and(|rules| rules.rules.contains_key(&rule)) {
                    return Err(rule_unknown());
                }
                rule
            }
            RuleSlot::NewRule => Uuid::new_v4(),
        };
        if pending.len() >= PENDING_CAPACITY {
            return Err(PublicError::invalid_input(
                "client.review_limit",
                "Too many requests are waiting for confirmation.",
            ));
        }
        let entry = PendingRule {
            rule_id,
            body,
            command_id: Uuid::new_v4(),
            token,
        };
        pending.insert((binding, slot), entry.clone());
        Ok(Claim {
            slot,
            pending: entry,
            fresh: true,
        })
    }

    /// The change resolved definitely: its slot is free again.
    fn release(&self, binding: PlaneBinding, claim: &Claim) -> Result<(), PublicError> {
        let mut pending = self.pending.lock().map_err(|_| PublicError::internal())?;
        if pending
            .get(&(binding, claim.slot))
            .is_some_and(|entry| entry.command_id == claim.pending.command_id)
        {
            pending.remove(&(binding, claim.slot));
        }
        Ok(())
    }

    /// Unconfirmed changes on this Plane, for the view.
    fn pending_views(&self, binding: PlaneBinding) -> Result<Vec<PendingChangeView>, PublicError> {
        let pending = self.pending.lock().map_err(|_| PublicError::internal())?;
        let mut views: Vec<PendingChangeView> = pending
            .iter()
            .filter(|((held, _), _)| *held == binding)
            .filter_map(|((_, slot), entry)| pending_view(*slot, entry))
            .collect();
        views.sort_by(|left, right| left.rule_id.cmp(&right.rule_id));
        Ok(views)
    }

    /// Rebuilds a Plane's tokens from a full read, keeping each token whose
    /// rule still reads the same.
    fn refresh_tokens(
        &self,
        binding: PlaneBinding,
        rules: &[&AutodeleteRule],
    ) -> Result<HashMap<Uuid, RuleTokens>, PublicError> {
        let mut all = self.tokens.lock().map_err(|_| PublicError::internal())?;
        let previous = all
            .remove(&binding.plane)
            .filter(|held| held.binding == binding)
            .map(|held| held.rules)
            .unwrap_or_default();
        let next: HashMap<Uuid, RuleTokens> = rules
            .iter()
            .map(|rule| {
                let current = RuleBinding::of(rule);
                let tokens = match previous.get(&rule.rule_id) {
                    Some(tokens) if tokens.binding == current => tokens.clone(),
                    _ => RuleTokens::issue(current),
                };
                (rule.rule_id, tokens)
            })
            .collect();
        all.insert(
            binding.plane,
            PlaneRules {
                binding,
                rules: next.clone(),
            },
        );
        Ok(next)
    }

    /// Records one rule a Command returned.
    fn record_rule(
        &self,
        binding: PlaneBinding,
        rule: &AutodeleteRule,
    ) -> Result<RuleTokens, PublicError> {
        let mut all = self.tokens.lock().map_err(|_| PublicError::internal())?;
        let held = all.entry(binding.plane).or_insert_with(|| PlaneRules {
            binding,
            rules: HashMap::new(),
        });
        if held.binding != binding {
            *held = PlaneRules {
                binding,
                rules: HashMap::new(),
            };
        }
        let current = RuleBinding::of(rule);
        let tokens = match held.rules.get(&rule.rule_id) {
            Some(tokens) if tokens.binding == current => tokens.clone(),
            _ => RuleTokens::issue(current),
        };
        held.rules.insert(rule.rule_id, tokens.clone());
        Ok(tokens)
    }

    fn forget_rule(&self, binding: PlaneBinding, rule: Uuid) -> Result<(), PublicError> {
        let mut all = self.tokens.lock().map_err(|_| PublicError::internal())?;
        if let Some(held) = all
            .get_mut(&binding.plane)
            .filter(|held| held.binding == binding)
        {
            held.rules.remove(&rule);
        }
        Ok(())
    }

    fn cached_attribution(
        &self,
        binding: PlaneBinding,
        job: Uuid,
    ) -> Result<Option<JobAttribution>, PublicError> {
        Ok(self
            .attributions
            .lock()
            .map_err(|_| PublicError::internal())?
            .get(&(binding, job))
            .cloned())
    }

    fn cache_attribution(
        &self,
        binding: PlaneBinding,
        job: Uuid,
        attribution: JobAttribution,
    ) -> Result<(), PublicError> {
        let mut cache = self
            .attributions
            .lock()
            .map_err(|_| PublicError::internal())?;
        if cache.len() >= ATTRIBUTION_CAPACITY {
            cache.clear();
        }
        cache.insert((binding, job), attribution);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// One change to one rule. Inner fields stay snake_case, like other input
/// enums.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AutodeleteChange {
    Compile {
        rule_id: Option<String>,
        prompt: String,
    },
    SetInactiveDays {
        rule_id: String,
        inactive_days: u32,
    },
    Approve {
        token_id: String,
    },
    AuthorizeEverywhere {
        token_id: String,
    },
    Delete {
        rule_id: String,
    },
}

fn parse_rule(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| rule_unknown())
}

fn parse_token(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| token_expired())
}

/// 1 to 4096 UTF-8 bytes of text; line breaks and tabs are the only
/// control characters kept.
fn valid_prompt(prompt: &str) -> bool {
    !prompt.is_empty()
        && prompt.len() <= PROMPT_LIMIT
        && prompt
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
}

fn parse_change(change: AutodeleteChange) -> Result<ParsedChange, PublicError> {
    Ok(match change {
        AutodeleteChange::Compile { rule_id, prompt } => {
            if !valid_prompt(&prompt) {
                return Err(PublicError::invalid_input(
                    "autodelete.prompt_invalid",
                    "Describe the rule in 1 to 4,096 bytes of text.",
                ));
            }
            ParsedChange::Compile {
                rule: rule_id.as_deref().map(parse_rule).transpose()?,
                prompt,
            }
        }
        AutodeleteChange::SetInactiveDays {
            rule_id,
            inactive_days,
        } => {
            if !DAYS_RANGE.contains(&inactive_days) {
                return Err(PublicError::invalid_input(
                    "autodelete.days_invalid",
                    "Choose from 1 to 36,500 days.",
                ));
            }
            ParsedChange::SetDays {
                rule: parse_rule(&rule_id)?,
                days: inactive_days,
            }
        }
        AutodeleteChange::Approve { token_id } => ParsedChange::Approve {
            token: parse_token(&token_id)?,
        },
        AutodeleteChange::AuthorizeEverywhere { token_id } => ParsedChange::Everywhere {
            token: parse_token(&token_id)?,
        },
        AutodeleteChange::Delete { rule_id } => ParsedChange::Delete {
            rule: parse_rule(&rule_id)?,
        },
    })
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutodeleteRulesView {
    plane_id: String,
    plane_label: String,
    cursor: String,
    drafting: DraftingView,
    rules: Vec<RuleView>,
    /// Changes Jet sent but could not confirm; "Try again" resends them.
    pending: Vec<PendingChangeView>,
}

/// Whether the Plane drafts rules with its Utility Provider. `None` when
/// the Setting could not be read. The binding itself never crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftingView {
    enabled: Option<bool>,
    binding_configured: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuleView {
    rule_id: String,
    /// The rule's wording, rendered as text; empty when the Plane's text
    /// was not displayable.
    prompt: String,
    state: RuleStateView,
    scope: &'static str,
    created_at_unix_ms: String,
    updated_at_unix_ms: String,
    candidates: Vec<CandidateView>,
    /// The Plane listed its limit of 32; more tasks may match.
    candidates_capped: bool,
    /// "Drafted by {model} via {provider}", only when the model drafted
    /// the interpretation shown.
    attribution: Option<AttributionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateView {
    conversation_id: String,
    last_active_at_unix_ms: String,
    protections: Vec<ProtectionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttributionView {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum RuleStateView {
    Compiling,
    Refused {
        reason: String,
    },
    Draft {
        inactive_days: u32,
        approve_token: String,
    },
    Approved {
        inactive_days: u32,
        approved_at_unix_ms: String,
        /// Only for a Forget rule: authorizes delete everywhere.
        everywhere_token: Option<String>,
    },
}

/// An unconfirmed change, in the shape the webview sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingChangeView {
    /// `None` for a new rule's first draft.
    rule_id: Option<String>,
    change: ChangeView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ChangeView {
    Compile {
        rule_id: Option<String>,
        prompt: String,
    },
    SetInactiveDays {
        rule_id: String,
        inactive_days: u32,
    },
    Approve {
        token_id: String,
    },
    AuthorizeEverywhere {
        token_id: String,
    },
    Delete {
        rule_id: String,
    },
}

fn pending_view(slot: RuleSlot, entry: &PendingRule) -> Option<PendingChangeView> {
    let rule_id = match slot {
        RuleSlot::Rule(rule) => Some(rule.to_string()),
        RuleSlot::NewRule => None,
    };
    let change = match &entry.body {
        RuleBody::Compile { prompt } => ChangeView::Compile {
            rule_id: rule_id.clone(),
            prompt: prompt.clone(),
        },
        RuleBody::SetDays { days } => ChangeView::SetInactiveDays {
            rule_id: entry.rule_id.to_string(),
            inactive_days: *days,
        },
        RuleBody::Approve { .. } => ChangeView::Approve {
            token_id: entry.token?.to_string(),
        },
        RuleBody::Everywhere => ChangeView::AuthorizeEverywhere {
            token_id: entry.token?.to_string(),
        },
        RuleBody::Delete => ChangeView::Delete {
            rule_id: entry.rule_id.to_string(),
        },
    };
    Some(PendingChangeView { rule_id, change })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RuleChangeOutcome {
    Recorded { rule: RuleView },
    Deleted { rule_id: String },
    Refused { error: PublicError },
}

fn scope_name(scope: AutodeleteScope) -> &'static str {
    match scope {
        AutodeleteScope::Forget => "forget",
        AutodeleteScope::Everywhere => "everywhere",
    }
}

fn state_view(
    state: &AutodeleteRuleState,
    tokens: &RuleTokens,
) -> Result<RuleStateView, PublicError> {
    Ok(match state {
        AutodeleteRuleState::Compiling => RuleStateView::Compiling,
        AutodeleteRuleState::Refused { reason } => RuleStateView::Refused {
            reason: safe_code(reason).unwrap_or_else(|| "autodelete.refused".into()),
        },
        AutodeleteRuleState::Draft { inactive_days } => RuleStateView::Draft {
            inactive_days: *inactive_days,
            approve_token: tokens
                .approve
                .ok_or_else(PublicError::internal)?
                .to_string(),
        },
        AutodeleteRuleState::Approved {
            inactive_days,
            approved_at_unix_ms,
        } => RuleStateView::Approved {
            inactive_days: *inactive_days,
            approved_at_unix_ms: approved_at_unix_ms.to_string(),
            everywhere_token: tokens.everywhere.map(|token| token.to_string()),
        },
    })
}

fn candidate_view(candidate: &AutodeleteCandidate) -> CandidateView {
    CandidateView {
        conversation_id: candidate.conversation_id.to_string(),
        last_active_at_unix_ms: candidate.last_active_at_unix_ms.to_string(),
        protections: candidate
            .protections
            .iter()
            .copied()
            .map(protection_view)
            .collect(),
    }
}

/// The attribution shown for a rule: only when the model drafted exactly
/// the inactivity the rule reads now, and names its Provider and Model.
fn attribution_view(
    rule: &AutodeleteRule,
    job: Option<&JobAttribution>,
) -> Option<AttributionView> {
    let days = match rule.state {
        AutodeleteRuleState::Draft { inactive_days }
        | AutodeleteRuleState::Approved { inactive_days, .. } => inactive_days,
        AutodeleteRuleState::Compiling | AutodeleteRuleState::Refused { .. } => return None,
    };
    let job = job?;
    (job.drafted_days == Some(days)).then_some(())?;
    Some(AttributionView {
        provider: job.provider.clone()?,
        model: job.model.clone()?,
    })
}

fn rule_view(
    rule: &AutodeleteRule,
    candidates: &[AutodeleteCandidate],
    tokens: &RuleTokens,
    attribution: Option<AttributionView>,
) -> Result<RuleView, PublicError> {
    if candidates.len() > CANDIDATES_LIMIT {
        return Err(PublicError::internal());
    }
    Ok(RuleView {
        rule_id: rule.rule_id.to_string(),
        prompt: if valid_prompt(&rule.prompt) {
            rule.prompt.clone()
        } else {
            String::new()
        },
        state: state_view(&rule.state, tokens)?,
        scope: scope_name(rule.scope),
        created_at_unix_ms: rule.created_at_unix_ms.to_string(),
        updated_at_unix_ms: rule.updated_at_unix_ms.to_string(),
        candidates: candidates.iter().map(candidate_view).collect(),
        candidates_capped: candidates.len() == CANDIDATES_LIMIT,
        attribution,
    })
}

/// Bounded display text, or nothing.
fn clean(value: Option<&str>, limit: usize) -> Option<String> {
    let value = value?;
    (!value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn load_autodelete_rules(
    bridge: State<'_, JetBridge>,
    plane_id: String,
) -> Result<AutodeleteRulesView, PublicError> {
    load_rules_for(&bridge, &plane_id).await
}

#[tauri::command]
pub(crate) async fn change_autodelete_rule(
    bridge: State<'_, JetBridge>,
    plane_id: String,
    change: AutodeleteChange,
) -> Result<RuleChangeOutcome, PublicError> {
    change_rule_for(&bridge, &plane_id, change).await
}

pub(in crate::jet) async fn load_rules_for(
    bridge: &JetBridge,
    plane_id: &str,
) -> Result<AutodeleteRulesView, PublicError> {
    let (binding, client) = bridge.plane(Some(plane_id))?;
    let plane = binding.plane;
    async {
        let connection = client
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let listed = connection
            .autodelete_rules()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        check_rules(&listed)?;
        bridge.planes.observe_success(plane, AUTODELETE_MINOR);
        let drafting = drafting(&connection).await;
        let attributions = attributions(bridge, binding, &connection, &listed).await?;
        let state = &bridge.retention.rules;
        let rules: Vec<&AutodeleteRule> =
            listed.rules.iter().map(|preview| &preview.rule).collect();
        let tokens = state.refresh_tokens(binding, &rules)?;
        let views = listed
            .rules
            .iter()
            .map(|preview| {
                let rule = &preview.rule;
                let tokens = tokens
                    .get(&rule.rule_id)
                    .ok_or_else(PublicError::internal)?;
                let attribution = attribution_view(rule, attributions.get(&rule.utility_job_id));
                rule_view(rule, &preview.candidates, tokens, attribution)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AutodeleteRulesView {
            plane_id: plane.to_string(),
            plane_label: plane_label(bridge, plane),
            cursor: listed.cursor.to_string(),
            drafting,
            rules: views,
            pending: state.pending_views(binding)?,
        })
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

/// The Plane's bounds, and one entry per rule.
fn check_rules(listed: &AutodeleteRules) -> Result<(), PublicError> {
    let mut seen = std::collections::HashSet::new();
    let valid = listed.rules.len() <= RULES_LIMIT
        && listed.rules.iter().all(|preview| {
            preview.candidates.len() <= CANDIDATES_LIMIT && seen.insert(preview.rule.rule_id)
        });
    if valid {
        Ok(())
    } else {
        Err(PublicError::internal())
    }
}

/// The drafting disclosure, on the same connection. A failed read is
/// `None`; the account binding's value is reduced to whether one is set.
async fn drafting(connection: &Client) -> DraftingView {
    let enabled = match plane_setting(connection, SettingKey::UtilityAutodeleteCompilation).await {
        Some(SettingValue::Flag(enabled)) => Some(enabled),
        _ => None,
    };
    let binding_configured =
        match plane_setting(connection, SettingKey::UtilityAccountBinding).await {
            Some(SettingValue::Text(binding)) => Some(!binding.is_empty()),
            _ => None,
        };
    DraftingView {
        enabled,
        binding_configured,
    }
}

/// Attribution for the rules whose interpretation a model may have drafted:
/// cached answers first, then at most 64 `utility` Queries on the same
/// connection. A failed lookup only leaves the attribution out.
async fn attributions(
    bridge: &JetBridge,
    binding: PlaneBinding,
    connection: &Client,
    listed: &AutodeleteRules,
) -> Result<HashMap<Uuid, JobAttribution>, PublicError> {
    let state = &bridge.retention.rules;
    let mut found = HashMap::new();
    let mut lookups = 0;
    for preview in &listed.rules {
        let rule = &preview.rule;
        let drafted = matches!(
            rule.state,
            AutodeleteRuleState::Draft { .. } | AutodeleteRuleState::Approved { .. }
        );
        let job = rule.utility_job_id;
        if !drafted || found.contains_key(&job) {
            continue;
        }
        if let Some(cached) = state.cached_attribution(binding, job)? {
            found.insert(job, cached);
            continue;
        }
        if lookups == ATTRIBUTION_LOOKUPS {
            continue;
        }
        lookups += 1;
        let Ok(answer) = connection.utility(job).await else {
            continue;
        };
        if answer.job_id != job || answer.purpose != UtilityPurpose::Autodelete {
            continue;
        }
        let attribution = JobAttribution {
            provider: clean(answer.provider.as_deref(), PROVIDER_LIMIT),
            model: clean(answer.model.as_deref(), MODEL_LIMIT),
            drafted_days: match answer.outcome {
                UtilityOutcome::Draft { inactive_days } => Some(inactive_days),
                _ => None,
            },
        };
        if answer.outcome != UtilityOutcome::Pending {
            state.cache_attribution(binding, job, attribution.clone())?;
        }
        found.insert(job, attribution);
    }
    Ok(found)
}

pub(in crate::jet) async fn change_rule_for(
    bridge: &JetBridge,
    plane_id: &str,
    change: AutodeleteChange,
) -> Result<RuleChangeOutcome, PublicError> {
    // Nothing is sent until the claim: every refusal before it is definite.
    let refused = |error: PublicError| RuleChangeOutcome::Refused {
        error: error.with_plane(plane_id.to_owned()),
    };
    let parsed = match parse_change(change) {
        Ok(parsed) => parsed,
        Err(error) => return Ok(refused(error)),
    };
    let (binding, client) = match bridge.plane(Some(plane_id)) {
        Ok(resolved) => resolved,
        Err(error) => return Ok(refused(error)),
    };
    let state = &bridge.retention.rules;
    let claim = match state.claim(binding, parsed) {
        Ok(claim) => claim,
        Err(error) => return Ok(refused(error)),
    };
    let settled = |error: PublicError| bridge.settle(&binding, error);
    let connection = match client.connect().await {
        Ok(connection) => connection,
        // A first send that never connected sent nothing; a resend may
        // follow an earlier one that the Plane applied.
        Err(error) if claim.fresh => {
            state.release(binding, &claim)?;
            return Ok(RuleChangeOutcome::Refused {
                error: settled(PublicError::from_client(&error)),
            });
        }
        Err(error) => return Err(settled(PublicError::from_client(&error))),
    };
    let entry = &claim.pending;
    let result = send(&connection, entry).await;
    match result {
        Ok(Some(rule)) => {
            if rule.rule_id != entry.rule_id {
                return Err(PublicError::internal());
            }
            state.release(binding, &claim)?;
            bridge
                .planes
                .observe_success(binding.plane, AUTODELETE_MINOR);
            let tokens = state.record_rule(binding, &rule)?;
            Ok(RuleChangeOutcome::Recorded {
                rule: rule_view(&rule, &[], &tokens, None)?,
            })
        }
        Ok(None) => {
            state.release(binding, &claim)?;
            state.forget_rule(binding, entry.rule_id)?;
            bridge
                .planes
                .observe_success(binding.plane, AUTODELETE_MINOR);
            Ok(RuleChangeOutcome::Deleted {
                rule_id: entry.rule_id.to_string(),
            })
        }
        Err(error) => {
            let public = PublicError::from_client(&error);
            if !definite(error.as_ref(), &public) {
                return Err(settled(public));
            }
            state.release(binding, &claim)?;
            if public.code == "autodelete.not_found" {
                state.forget_rule(binding, entry.rule_id)?;
            }
            Ok(RuleChangeOutcome::Refused {
                error: settled(public),
            })
        }
    }
}

/// Sends one change under its Command ID. `None` means the rule is deleted.
async fn send(
    connection: &Client,
    entry: &PendingRule,
) -> Result<Option<AutodeleteRule>, Box<ClientError>> {
    let (command, rule) = (entry.command_id, entry.rule_id);
    let result = match &entry.body {
        RuleBody::Compile { prompt } => connection
            .compile_autodelete_rule(command, rule, prompt.clone())
            .await
            .map(Some),
        RuleBody::SetDays { days } => connection
            .set_autodelete_rule_inactive_days(command, rule, *days)
            .await
            .map(Some),
        RuleBody::Approve { days } => connection
            .approve_autodelete_rule(command, rule, *days)
            .await
            .map(Some),
        RuleBody::Everywhere => connection
            .authorize_autodelete_everywhere(command, rule)
            .await
            .map(Some),
        RuleBody::Delete => connection
            .delete_autodelete_rule(command, rule)
            .await
            .map(|()| None),
    };
    result.map_err(Box::new)
}

#[cfg(test)]
mod tests;
