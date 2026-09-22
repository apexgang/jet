//! Reviewed, typed Git delivery. The daemon owns Git and the durable outbox.
use std::{collections::HashMap, sync::Mutex};

use jet_protocol::{
    CommandRequest, CommandResponse, DiffScope, GitCheckpoint, GitDelivery, GitDeliveryOutcome,
    GitOperation, WorkingTree,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use super::{errors::PublicError, JetBridge};

#[derive(Default)]
pub(crate) struct DeliveryState {
    reviews: Mutex<HashMap<Uuid, Review>>,
}

struct Review {
    order: u64,
    conversation_id: Uuid,
    command: CommandRequest,
    attempted: bool,
    result: Option<DeliveryReceipt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum DeliveryReceipt {
    Accepted { delivery_id: String },
    Refused { error: PublicError },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DeliveryOperation {
    Branch {
        name: String,
    },
    Commit {
        run_id: String,
        turn: u32,
    },
    Push {
        remote: String,
    },
    DraftPullRequest {
        run_id: String,
        turn: u32,
        remote: String,
        base: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryView {
    id: String,
    operation: &'static str,
    destination: String,
    checkpoint: Option<String>,
    status: &'static str,
    head: Option<String>,
    branch: Option<String>,
    pull_request: Option<String>,
    code: Option<String>,
    acknowledged: bool,
    title: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryReview {
    review_id: String,
    conversation_id: String,
    working_tree: &'static str,
    operation: DeliveryOperation,
    checkpoint_files: Option<u32>,
    content_complete: Option<bool>,
}

#[tauri::command]
pub(crate) async fn load_deliveries(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
) -> Result<Vec<DeliveryView>, PublicError> {
    let id = parse_id(&conversation_id)?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    let deliveries = client
        .git_deliveries(id)
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    if deliveries.len() > 100 || deliveries.iter().any(|d| d.conversation_id != id) {
        return Err(PublicError::internal());
    }
    Ok(deliveries.into_iter().map(delivery_view).collect())
}

#[tauri::command]
pub(crate) async fn prepare_delivery(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    operation: DeliveryOperation,
) -> Result<DeliveryReview, PublicError> {
    let id = parse_id(&conversation_id)?;
    let (operation_wire, checkpoint) = operation_wire(&operation)?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    // The typed read checks protocol support before admitting any mutation.
    client
        .git_deliveries(id)
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    let snapshot = client
        .conversation(id)
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    let working_tree = match snapshot
        .conversation
        .working_tree
        .unwrap_or(WorkingTree::NoProject)
    {
        WorkingTree::NoProject => {
            return Err(invalid(
                "delivery.no_project",
                "Choose a Conversation with a Project.",
            ))
        }
        WorkingTree::Workspace { .. } => "Managed Workspace",
        WorkingTree::LocalCheckout { .. } => "Local checkout",
    };
    let (checkpoint_files, content_complete) = if let Some(checkpoint) = checkpoint {
        if !snapshot.runs.iter().any(|r| r.run_id == checkpoint.run_id) {
            return Err(invalid(
                "delivery.checkpoint_owner",
                "Choose a checkpoint from this Conversation.",
            ));
        }
        let diff = client
            .change_diff(
                checkpoint.run_id,
                DiffScope::Turn {
                    turn: checkpoint.turn,
                },
            )
            .await
            .map_err(|e| PublicError::from_client(&e))?;
        (Some(diff.total_files), Some(diff.after.content_complete))
    } else {
        (None, None)
    };
    let command = CommandRequest::DeliverGit {
        conversation_id: id,
        checkpoint,
        operation: operation_wire,
    };
    let review_id = bridge.delivery.prepare(id, command)?;
    Ok(DeliveryReview {
        review_id: review_id.to_string(),
        conversation_id,
        working_tree,
        operation,
        checkpoint_files,
        content_complete,
    })
}

impl DeliveryState {
    fn prepare(&self, conversation_id: Uuid, command: CommandRequest) -> Result<Uuid, PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        if reviews
            .values()
            .any(|r| r.conversation_id == conversation_id && r.attempted && r.result.is_none())
        {
            return Err(invalid(
                "delivery.request_unresolved",
                "Resolve the previous delivery request before starting another.",
            ));
        }
        // Each review is immutable, including overlapping read requests. A late
        // preview must not expire a newer preview that the user is reviewing.
        // Attempted requests retain their ID and exact body across reconnects.
        let order = reviews
            .values()
            .map(|review| review.order)
            .max()
            .unwrap_or(0)
            + 1;
        if reviews.len() >= 256 {
            let oldest = reviews
                .iter()
                .filter(|(_, review)| !review.attempted)
                .min_by_key(|(_, review)| review.order)
                .map(|(id, _)| *id);
            if let Some(id) = oldest {
                reviews.remove(&id);
            } else {
                return Err(invalid(
                    "delivery.request_limit",
                    "Too many submitted delivery requests are retained. Restart Jet after resolving pending work.",
                ));
            }
        }
        let id = Uuid::new_v4();
        reviews.insert(
            id,
            Review {
                order,
                conversation_id,
                command,
                attempted: false,
                result: None,
            },
        );
        Ok(id)
    }

    fn attempt(&self, id: Uuid) -> Result<(CommandRequest, Option<DeliveryReceipt>), PublicError> {
        let mut reviews = self.reviews.lock().map_err(|_| PublicError::internal())?;
        let conversation_id = reviews
            .get(&id)
            .ok_or_else(|| {
                invalid(
                    "delivery.review_expired",
                    "Review this operation again before sending it.",
                )
            })?
            .conversation_id;
        if reviews.iter().any(|(other_id, review)| {
            *other_id != id
                && review.conversation_id == conversation_id
                && review.attempted
                && review.result.is_none()
        }) {
            return Err(invalid(
                "delivery.request_unresolved",
                "Resolve the previous delivery request before starting another.",
            ));
        }
        let review = reviews.get_mut(&id).ok_or_else(PublicError::internal)?;
        review.attempted = true;
        Ok((review.command.clone(), review.result.clone()))
    }
}

#[tauri::command]
pub(crate) async fn execute_delivery(
    bridge: State<'_, JetBridge>,
    review_id: String,
) -> Result<DeliveryReceipt, PublicError> {
    let id = parse_id(&review_id)?;
    let (command, known) = match bridge.delivery.attempt(id) {
        Ok(attempt) => attempt,
        Err(error) => return Ok(DeliveryReceipt::Refused { error }),
    };
    if let Some(known) = known {
        return Ok(known);
    }
    let expected_acknowledgement =
        if let CommandRequest::AcknowledgeGitDelivery { delivery_id } = &command {
            Some(*delivery_id)
        } else {
            None
        };
    let result = bridge
        .client
        .connect()
        .await
        .map_err(|e| PublicError::from_client(&e))?
        .execute_command(id, command)
        .await;
    if let Ok(ref response) = result {
        let matches = match (expected_acknowledgement, response) {
            (None, CommandResponse::GitDeliveryQueued { .. }) => true,
            (Some(expected), CommandResponse::GitDeliveryAcknowledged { delivery_id }) => {
                expected == *delivery_id
            }
            _ => false,
        };
        if !matches {
            return Err(PublicError::internal());
        }
    }
    match result {
        Ok(CommandResponse::GitDeliveryQueued { delivery_id })
        | Ok(CommandResponse::GitDeliveryAcknowledged { delivery_id }) => {
            let value = DeliveryReceipt::Accepted {
                delivery_id: delivery_id.to_string(),
            };
            if let Some(review) = bridge
                .delivery
                .reviews
                .lock()
                .map_err(|_| PublicError::internal())?
                .get_mut(&id)
            {
                review.result = Some(value.clone());
            }
            Ok(value)
        }
        Ok(_) => Err(PublicError::internal()),
        Err(error) => {
            let public = PublicError::from_client(&error);
            // A definite daemon rejection ends admission. Transport/decoding
            // errors remain uncertain and must reuse this exact request.
            if matches!(error, jet_client::ClientError::Remote(_))
                && public.category != "outcome_unknown"
            {
                let receipt = DeliveryReceipt::Refused { error: public };
                if let Some(review) = bridge
                    .delivery
                    .reviews
                    .lock()
                    .map_err(|_| PublicError::internal())?
                    .get_mut(&id)
                {
                    review.result = Some(receipt.clone());
                }
                return Ok(receipt);
            }
            Err(public)
        }
    }
}

#[tauri::command]
pub(crate) async fn prepare_delivery_acknowledgement(
    bridge: State<'_, JetBridge>,
    conversation_id: String,
    delivery_id: String,
) -> Result<String, PublicError> {
    let conversation_id = parse_id(&conversation_id)?;
    let delivery_id = parse_id(&delivery_id)?;
    let client = bridge
        .client
        .connect()
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    let rows = client
        .git_deliveries(conversation_id)
        .await
        .map_err(|e| PublicError::from_client(&e))?;
    if !rows.iter().any(|d| {
        d.delivery_id == delivery_id
            && d.conversation_id == conversation_id
            && d.acknowledged_by.is_none()
            && matches!(d.outcome, GitDeliveryOutcome::OutcomeUnknown)
    }) {
        return Err(invalid(
            "delivery.acknowledgement_invalid",
            "Refresh delivery history and inspect the uncertain operation first.",
        ));
    }
    bridge
        .delivery
        .prepare(
            conversation_id,
            CommandRequest::AcknowledgeGitDelivery { delivery_id },
        )
        .map(|id| id.to_string())
}

fn operation_wire(
    operation: &DeliveryOperation,
) -> Result<(GitOperation, Option<GitCheckpoint>), PublicError> {
    let checkpoint = |run_id: &str, turn: u32| {
        if turn == 0 {
            return Err(invalid(
                "delivery.turn_invalid",
                "Choose a retained Turn numbered from one.",
            ));
        }
        Ok(Some(GitCheckpoint {
            run_id: parse_id(run_id)?,
            turn,
        }))
    };
    match operation {
        DeliveryOperation::Branch { name } => {
            git_name(name)?;
            Ok((GitOperation::Branch { name: name.clone() }, None))
        }
        DeliveryOperation::Commit { run_id, turn } => {
            Ok((GitOperation::Commit, checkpoint(run_id, *turn)?))
        }
        DeliveryOperation::Push { remote } => {
            git_name(remote)?;
            Ok((
                GitOperation::Push {
                    remote: remote.clone(),
                },
                None,
            ))
        }
        DeliveryOperation::DraftPullRequest {
            run_id,
            turn,
            remote,
            base,
        } => {
            git_name(remote)?;
            git_name(base)?;
            Ok((
                GitOperation::DraftPullRequest {
                    remote: remote.clone(),
                    base: Some(base.clone()),
                },
                checkpoint(run_id, *turn)?,
            ))
        }
    }
}

fn git_name(value: &str) -> Result<(), PublicError> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid(
            "delivery.name_invalid",
            "Enter a Git name of 1–255 characters, without spaces or a leading dash.",
        ));
    }
    Ok(())
}
fn parse_id(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| {
        invalid(
            "delivery.identifier_invalid",
            "Choose a valid delivery or Conversation.",
        )
    })
}
fn invalid(code: &'static str, message: &'static str) -> PublicError {
    PublicError::invalid_input(code, message)
}
fn bounded(value: String, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

fn delivery_view(row: GitDelivery) -> DeliveryView {
    let (operation, destination) = match row.operation {
        GitOperation::Branch { name } => ("branch", name),
        GitOperation::Commit => ("commit", "Conversation working tree".into()),
        GitOperation::Push { remote } => ("push", remote),
        GitOperation::DraftPullRequest { remote, base } => (
            "draft_pull_request",
            format!(
                "{remote} / {}",
                base.as_deref().unwrap_or("repository default")
            ),
        ),
    };
    let (status, head, branch, pull_request, code) = match row.outcome {
        GitDeliveryOutcome::Pending => ("pending", None, None, None, None),
        GitDeliveryOutcome::Completed {
            head,
            branch,
            pull_request,
        } => (
            "completed",
            Some(bounded(head, 64)),
            branch.map(|b| bounded(b, 255)),
            pull_request.filter(|url| safe_pr_url(url)),
            None,
        ),
        GitDeliveryOutcome::Failed { code } => {
            ("failed", None, None, None, Some(bounded(code, 128)))
        }
        GitDeliveryOutcome::OutcomeUnknown => ("outcome_unknown", None, None, None, None),
    };
    DeliveryView {
        id: row.delivery_id.to_string(),
        operation,
        destination: bounded(destination, 520),
        checkpoint: row
            .checkpoint
            .map(|c| format!("{} / Turn {}", c.run_id, c.turn)),
        status,
        head,
        branch,
        pull_request,
        code,
        acknowledged: row.acknowledged_by.is_some(),
        title: row.message.map(|m| bounded(m.title, 512)),
    }
}

fn safe_pr_url(value: &str) -> bool {
    let Some(path) = value.strip_prefix("https://github.com/") else {
        return false;
    };
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 4
        && parts[2] == "pull"
        && !parts[3].is_empty()
        && parts[3].bytes().all(|b| b.is_ascii_digit())
        && parts[..2].iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_keep_the_exact_request_and_block_replacement() {
        let state = DeliveryState::default();
        let conversation_id = Uuid::new_v4();
        let command = CommandRequest::DeliverGit {
            conversation_id,
            checkpoint: None,
            operation: GitOperation::Push {
                remote: "origin".into(),
            },
        };
        let id = state.prepare(conversation_id, command.clone()).unwrap();
        let (retried, known) = state.attempt(id).unwrap();
        assert_eq!(retried, command);
        assert!(known.is_none());
        let (retried, known) = state.attempt(id).unwrap();
        assert_eq!(retried, command);
        assert!(known.is_none());
        assert!(state.prepare(conversation_id, command).is_err());
    }
    #[test]
    fn overlapping_reviews_stay_valid_but_cannot_bypass_uncertainty() {
        let state = DeliveryState::default();
        let conversation_id = Uuid::new_v4();
        let command = CommandRequest::DeliverGit {
            conversation_id,
            checkpoint: None,
            operation: GitOperation::Push {
                remote: "origin".into(),
            },
        };
        let first = state.prepare(conversation_id, command.clone()).unwrap();
        let second = state.prepare(conversation_id, command.clone()).unwrap();
        assert_eq!(state.attempt(second).unwrap().0, command);
        assert!(state.attempt(first).is_err());
        assert_eq!(state.attempt(second).unwrap().0, command);
    }

    #[test]
    fn abandoned_previews_do_not_exhaust_capacity_or_evict_uncertain_requests() {
        let state = DeliveryState::default();
        let conversation_id = Uuid::new_v4();
        let command = CommandRequest::DeliverGit {
            conversation_id,
            checkpoint: None,
            operation: GitOperation::Commit,
        };
        let unresolved = state.prepare(conversation_id, command.clone()).unwrap();
        state.attempt(unresolved).unwrap();
        let completed = state.prepare(Uuid::new_v4(), command.clone()).unwrap();
        state.attempt(completed).unwrap();
        state
            .reviews
            .lock()
            .unwrap()
            .get_mut(&completed)
            .unwrap()
            .result = Some(DeliveryReceipt::Accepted {
            delivery_id: "accepted-before-ipc-loss".into(),
        });
        let other = Uuid::new_v4();
        let oldest = state.prepare(other, command.clone()).unwrap();
        for _ in 0..300 {
            state.prepare(other, command.clone()).unwrap();
        }
        assert!(state.attempt(oldest).is_err());
        assert_eq!(state.attempt(unresolved).unwrap().0, command);
        assert!(matches!(state.attempt(completed).unwrap().1,
            Some(DeliveryReceipt::Accepted { delivery_id }) if delivery_id == "accepted-before-ipc-loss"));
        assert_eq!(state.reviews.lock().unwrap().len(), 256);
    }

    #[test]
    fn checkpoint_and_inputs_are_bounded() {
        assert!(operation_wire(&DeliveryOperation::Commit {
            run_id: Uuid::new_v4().to_string(),
            turn: 0
        })
        .is_err());
        for value in ["", "-f", "origin\nsecret", "two names"] {
            assert!(git_name(value).is_err());
        }
        assert!(git_name("feature/delivery").is_ok());
    }
    #[test]
    fn only_github_pull_request_urls_cross_the_boundary() {
        assert!(safe_pr_url("https://github.com/apexgang/jet/pull/12"));
        for value in [
            "https://github.com.evil/a/b/pull/1",
            "https://token@github.com/a/b/pull/1",
            "javascript:alert(1)",
            "https://github.com/a/b/pull/1?token=private",
        ] {
            assert!(!safe_pr_url(value));
        }
    }
}
