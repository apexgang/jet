//! Command IDs the shell holds natively (ADR-0093). A Plane receipts a
//! Command's durable answer under its ID, refusals included, so reusing an
//! ID replays that answer. An ID is therefore kept only while the outcome is
//! uncertain (outcome unknown, a lost transport, an expired deadline), so a
//! retry resends the exact same request, and dropped on any definite answer,
//! so the next attempt is a new request.
//!
//! A kept ID must also never be sent for a different intent: the Plane would
//! replay the old answer instead of doing the new work. `PendingCommands`
//! therefore keeps one ID per target, tied to the exact request it was made
//! for, and callers forget it once the Plane's state shows it can no longer
//! apply.

use std::{
    borrow::Borrow,
    collections::HashMap,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

use jet_client::ClientError;
use jet_protocol::ErrorCategory;
use uuid::Uuid;

use super::{deadline::COMMAND_DEADLINE, errors::PublicError};

/// Drops a command ID once its outcome is known: on success or on a
/// definite refusal. An uncertain outcome keeps it for an exact retry.
pub(crate) fn settle_command<K: Hash + Eq, T, E: Borrow<ClientError>>(
    commands: &Mutex<HashMap<K, Uuid>>,
    key: &K,
    outcome: &Result<T, E>,
) -> Result<(), PublicError> {
    let known = match outcome {
        Ok(_) => true,
        Err(error) => definite(error.borrow()),
    };
    release_if(commands, key, known)
}

/// Drops `key`'s command ID when `known` says its outcome is settled.
pub(crate) fn release_if<K: Hash + Eq>(
    commands: &Mutex<HashMap<K, Uuid>>,
    key: &K,
    known: bool,
) -> Result<(), PublicError> {
    if known {
        commands
            .lock()
            .map_err(|_| PublicError::internal())?
            .remove(key);
    }
    Ok(())
}

/// Whether a failure settles the Command: the Plane's durable answer to it,
/// a stable refusal other than `outcome_unknown` (`execute.rs`: "An
/// authoritative error is a durable answer"), or a local protocol gate that
/// kept it from being sent on this connection at all.
///
/// Nothing else does. A lost transport or an expired deadline
/// (`deadline.rs`) may follow an applied Command. A refused handshake or an
/// incompatible protocol answers nothing about the Command, and
/// `PlaneClient::with_reconnect` can meet one on its reconnect after an
/// earlier attempt with the same ID was sent; that loop also reports any
/// failure after such an attempt as unconfirmed (`ConnectError::definite`).
/// Keeping an ID that was never sent costs nothing: its retry is simply the
/// first real send.
pub(crate) fn definite(error: &ClientError) -> bool {
    match error {
        ClientError::Remote(wire) => wire.category != ErrorCategory::OutcomeUnknown,
        ClientError::FeatureUnavailable { .. } => true,
        ClientError::Rejected(_)
        | ClientError::Incompatible { .. }
        | ClientError::Io(_)
        | ClientError::Frame(_)
        | ClientError::Control(_)
        | ClientError::Closed
        | ClientError::Disconnected(_)
        | ClientError::Unexpected(_) => false,
    }
}

/// How many targets may hold a kept Command ID at once.
const MAX_PENDING_COMMANDS: usize = 256;
/// A kept ID older than this can be evicted to make room: its Command has
/// had its whole deadline, so no request of this app still waits on it.
const EVICTABLE_AFTER: Duration = COMMAND_DEADLINE;

/// The Command IDs kept for one kind of request, at most one per target (the
/// Run, Conversation or Project it acts on).
///
/// The kept ID belongs to the exact request last attempted on its target
/// whose outcome is still uncertain. Asking again for that request returns
/// the same ID, so the retry is answered from the receipt instead of being
/// applied twice. Asking for any other request on the target (a later Turn,
/// another send) replaces it with a new ID: sent with a new intent, the old
/// ID would only replay the old answer.
pub(crate) struct PendingCommands<T, R> {
    pending: Mutex<HashMap<T, Kept<R>>>,
}

struct Kept<R> {
    request: R,
    id: Uuid,
    /// When the ID was made, for eviction once the map is full.
    made: Instant,
}

impl<T, R> Default for PendingCommands<T, R> {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Hash + Eq, R: PartialEq> PendingCommands<T, R> {
    /// The ID to send `request` on `target` under.
    pub(crate) fn id(&self, target: T, request: R) -> Result<Uuid, PublicError> {
        self.id_at(target, request, Instant::now())
    }

    /// `id` at `now`. When every slot is taken, the oldest kept ID that has
    /// outlived a Command's whole deadline is evicted (its retry would be a
    /// new request); only while all of them are younger is the request
    /// refused, so uncertain outcomes can no longer block new work forever.
    fn id_at(&self, target: T, request: R, now: Instant) -> Result<Uuid, PublicError> {
        let mut pending = self.pending.lock().map_err(|_| PublicError::internal())?;
        match pending.get(&target) {
            Some(kept) if kept.request == request => return Ok(kept.id),
            Some(_) => {}
            None if pending.len() >= MAX_PENDING_COMMANDS => {
                let oldest = pending
                    .values()
                    .filter(|kept| now.saturating_duration_since(kept.made) >= EVICTABLE_AFTER)
                    .min_by_key(|kept| kept.made)
                    .map(|kept| kept.id);
                let Some(oldest) = oldest else {
                    return Err(PublicError::invalid_input(
                        "client.too_many_pending_commands",
                        "Finish or retry an earlier action before starting another one.",
                    ));
                };
                pending.retain(|_, kept| kept.id != oldest);
            }
            None => {}
        }
        let id = Uuid::new_v4();
        pending.insert(
            target,
            Kept {
                request,
                id,
                made: now,
            },
        );
        Ok(id)
    }

    /// Drops `id` once `outcome` is known: on success or a definite refusal.
    /// An uncertain outcome keeps it for an exact retry, and a newer request
    /// that replaced it on `target` keeps its own.
    pub(crate) fn settle<V, E: Borrow<ClientError>>(
        &self,
        target: &T,
        id: Uuid,
        outcome: &Result<V, E>,
    ) -> Result<(), PublicError> {
        let known = match outcome {
            Ok(_) => true,
            Err(error) => definite(error.borrow()),
        };
        if known {
            let mut pending = self.pending.lock().map_err(|_| PublicError::internal())?;
            if pending.get(target).is_some_and(|kept| kept.id == id) {
                pending.remove(target);
            }
        }
        Ok(())
    }

    /// Forgets the IDs kept for every target `stale` matches, once the
    /// Plane's state shows that their requests have applied or no longer
    /// can: the next request there is new.
    pub(crate) fn forget(&self, stale: impl Fn(&T) -> bool) -> Result<(), PublicError> {
        self.pending
            .lock()
            .map_err(|_| PublicError::internal())?
            .retain(|target, _| !stale(target));
        Ok(())
    }

    /// Whether a kept ID's target and request match `kept`.
    pub(crate) fn holds(&self, kept: impl Fn(&T, &R) -> bool) -> Result<bool, PublicError> {
        Ok(self
            .pending
            .lock()
            .map_err(|_| PublicError::internal())?
            .iter()
            .any(|(target, held)| kept(target, &held.request)))
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use jet_client::ClientError;
    use jet_protocol::ErrorCategory;

    use super::{definite, PendingCommands};
    use crate::jet::fake_plane::wire;

    /// A refused handshake or an incompatible protocol answers nothing
    /// about a Command: `with_reconnect` meets one on a reconnect, after an
    /// earlier attempt with the same ID may have been sent.
    #[test]
    fn a_handshake_outcome_never_settles_a_command() {
        assert!(!definite(&ClientError::Rejected(wire(
            ErrorCategory::Unauthorized,
            "connection.unauthorized"
        ))));
        assert!(!definite(&ClientError::Incompatible {
            protocol: 9,
            minor: 0,
            codec: "json".into(),
        }));
        // A local gate sent nothing on this connection.
        assert!(definite(&ClientError::FeatureUnavailable {
            required_minor: 9,
            negotiated_minor: 1,
        }));
        assert!(!definite(&ClientError::Closed));
        assert!(definite(&ClientError::Remote(wire(
            ErrorCategory::InvalidInput,
            "account.provider_unknown"
        ))));
    }

    /// One kept ID per target, for its exact request: a retry gets it back,
    /// any other request on the target replaces it, and it is dropped once
    /// its outcome is known or the Plane shows it is stale.
    #[test]
    fn a_kept_id_is_reused_only_for_its_exact_request() {
        let commands = PendingCommands::<u8, &str>::default();
        let first = commands.id(1, "continue").unwrap();
        assert_eq!(commands.id(1, "continue").unwrap(), first);
        let other = commands.id(1, "continue, again").unwrap();
        assert_ne!(other, first, "a new intent never reuses a kept ID");
        // The replaced ID's late answer leaves the newer one alone.
        let answered: Result<(), ClientError> = Ok(());
        commands.settle(&1, first, &answered).unwrap();
        assert_eq!(commands.id(1, "continue, again").unwrap(), other);
        let unknown: Result<(), ClientError> = Err(ClientError::Closed);
        commands.settle(&1, other, &unknown).unwrap();
        assert_eq!(commands.id(1, "continue, again").unwrap(), other);
        commands.settle(&1, other, &answered).unwrap();
        assert!(commands.is_empty());

        let kept = commands.id(2, "stop").unwrap();
        commands.forget(|target| *target == 2).unwrap();
        assert_ne!(commands.id(2, "stop").unwrap(), kept);
    }

    /// Finding 9: uncertain IDs used to stay until a definite answer, so
    /// 256 of them refused every later Command for the rest of the session.
    /// When full, the oldest one past a Command's whole deadline makes room;
    /// younger ones still refuse.
    #[test]
    fn a_full_map_evicts_the_oldest_id_past_the_command_deadline() {
        use std::time::{Duration, Instant};

        use super::{EVICTABLE_AFTER, MAX_PENDING_COMMANDS};

        let commands = PendingCommands::<usize, &str>::default();
        let start = Instant::now();
        let first = commands.id_at(0, "old", start).unwrap();
        for target in 1..MAX_PENDING_COMMANDS {
            commands
                .id_at(target, "young", start + Duration::from_secs(1))
                .unwrap();
        }
        let soon = start + EVICTABLE_AFTER - Duration::from_secs(1);
        assert_eq!(
            commands
                .id_at(MAX_PENDING_COMMANDS, "new", soon)
                .unwrap_err()
                .code,
            "client.too_many_pending_commands"
        );
        let later = start + EVICTABLE_AFTER;
        let made = commands.id_at(MAX_PENDING_COMMANDS, "new", later).unwrap();
        assert_ne!(made, first);
        // The evicted target's retry is a new request now.
        let full_again = commands.id_at(0, "old", later).unwrap_err();
        assert_eq!(full_again.code, "client.too_many_pending_commands");
        let much_later = start + 2 * EVICTABLE_AFTER;
        assert_ne!(commands.id_at(0, "old", much_later).unwrap(), first);
        // A kept ID is still returned for its exact retry.
        assert_eq!(
            commands
                .id_at(MAX_PENDING_COMMANDS, "new", much_later)
                .unwrap(),
            made
        );
    }

    /// An expired deadline is never a definite answer: the Command may have
    /// reached the Plane.
    #[test]
    fn an_expired_deadline_keeps_the_command_id() {
        let expired = ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "deadline",
        ));
        assert!(!definite(&expired));
        assert!(definite(&ClientError::Remote(wire(
            ErrorCategory::Conflict,
            "turn.queue_full"
        ))));
        assert!(!definite(&ClientError::Remote(wire(
            ErrorCategory::OutcomeUnknown,
            "command.outcome_unknown"
        ))));
    }
}
