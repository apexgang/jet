//! A bounded, per-Plane ledger of native reviews (Wave 3.3 §4.2).
//!
//! A review is issued against one Plane and one scope (a Conversation, or a
//! fixed per-action UUID). Its ID is the daemon Command ID, so a lost IPC
//! reply is answered from the recorded outcome and an uncertain send is
//! resent with the same ID. While a review for a scope is attempted and
//! unresolved, no other review for that scope can be issued or attempted.
//!
//! Jet Trash reviews (`retention.rs`) and Recovery restore and purge reviews
//! (`system/recovery.rs`) build on this primitive.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use uuid::Uuid;

use super::{
    errors::PublicError,
    planes::{PlaneBinding, PlaneId},
};

/// How long an unattempted review, or a recorded outcome, stays usable.
/// Matches the preview grants in `setup.rs`.
pub(crate) const REVIEW_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// At most this many reviews are held at once (as `delivery.rs`).
pub(crate) const REVIEW_CAPACITY: usize = 256;

/// The stable codes a ledger reports in its own namespace.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LedgerCodes {
    pub(crate) review_expired: &'static str,
    pub(crate) request_unresolved: &'static str,
}

pub(crate) const RETENTION_CODES: LedgerCodes = LedgerCodes {
    review_expired: "retention.review_expired",
    request_unresolved: "retention.request_unresolved",
};

pub(crate) const RECOVERY_CODES: LedgerCodes = LedgerCodes {
    review_expired: "recovery.review_expired",
    request_unresolved: "recovery.request_unresolved",
};

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

pub(crate) struct Ledger<T, R> {
    inner: Mutex<Book<T, R>>,
    codes: LedgerCodes,
    lifetime: Duration,
    capacity: usize,
    now: Clock,
}

struct Book<T, R> {
    next_order: u64,
    entries: HashMap<Uuid, Entry<T, R>>,
}

struct Entry<T, R> {
    order: u64,
    created: Instant,
    binding: PlaneBinding,
    scope: Uuid,
    value: T,
    attempted: bool,
    result: Option<(R, Instant)>,
}

impl<T, R> Entry<T, R> {
    /// Attempted and still waiting for a definite outcome.
    fn unresolved(&self) -> bool {
        self.attempted && self.result.is_none()
    }

    fn expired(&self, now: Instant, lifetime: Duration) -> bool {
        match &self.result {
            Some((_, recorded)) => now.saturating_duration_since(*recorded) > lifetime,
            None => !self.attempted && now.saturating_duration_since(self.created) > lifetime,
        }
    }
}

/// What an attempt may do with a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Attempt<T, R> {
    /// The first attempt: the request has never been sent.
    Fresh(T),
    /// A later attempt of a request whose outcome is still unknown: resend
    /// it unchanged.
    Retry(T),
    /// The outcome was recorded: return it without contacting the Plane.
    Known(R),
}

impl<T: Clone, R: Clone> Ledger<T, R> {
    pub(crate) fn new(codes: LedgerCodes) -> Self {
        Self::with_clock(
            codes,
            REVIEW_LIFETIME,
            REVIEW_CAPACITY,
            Arc::new(Instant::now),
        )
    }

    pub(crate) fn with_clock(
        codes: LedgerCodes,
        lifetime: Duration,
        capacity: usize,
        now: Clock,
    ) -> Self {
        Self {
            inner: Mutex::new(Book {
                next_order: 0,
                entries: HashMap::new(),
            }),
            codes,
            lifetime,
            capacity,
            now,
        }
    }

    /// Issues a review of `value` for `scope` on the bound Plane and returns
    /// its ID, which is also the Command ID the request is sent with.
    pub(crate) fn issue(
        &self,
        binding: PlaneBinding,
        scope: Uuid,
        value: T,
    ) -> Result<Uuid, PublicError> {
        let now = (self.now)();
        let mut book = self.inner.lock().map_err(|_| PublicError::internal())?;
        let lifetime = self.lifetime;
        book.entries
            .retain(|_, entry| !entry.expired(now, lifetime));
        if book.entries.values().any(|entry| {
            entry.binding.plane == binding.plane && entry.scope == scope && entry.unresolved()
        }) {
            return Err(self.unresolved_error());
        }
        while book.entries.len() >= self.capacity {
            // Resolved outcomes go first, then the oldest unattempted review.
            // An attempted, unresolved review is never evicted: its outcome
            // is still owed to the user.
            let victim = oldest(&book.entries, |entry| entry.result.is_some())
                .or_else(|| oldest(&book.entries, |entry| !entry.attempted));
            match victim {
                Some(id) => {
                    book.entries.remove(&id);
                }
                None => {
                    return Err(PublicError::invalid_input(
                        "client.review_limit",
                        "Too many requests are waiting for confirmation.",
                    ))
                }
            }
        }
        let id = Uuid::new_v4();
        let order = book.next_order;
        book.next_order += 1;
        book.entries.insert(
            id,
            Entry {
                order,
                created: now,
                binding,
                scope,
                value,
                attempted: false,
                result: None,
            },
        );
        Ok(id)
    }

    /// Attempts review `id` against `plane`. `check` runs before the review
    /// is marked attempted, and can lock or refuse on the stored value (for
    /// example a mode lock or a required acknowledgement). Returns the Plane
    /// binding the review was issued against with the attempt.
    pub(crate) fn attempt(
        &self,
        id: Uuid,
        plane: PlaneId,
        check: impl FnOnce(&mut T) -> Result<(), PublicError>,
    ) -> Result<(PlaneBinding, Attempt<T, R>), PublicError> {
        let now = (self.now)();
        let mut book = self.inner.lock().map_err(|_| PublicError::internal())?;
        let Some(entry) = book.entries.get(&id) else {
            return Err(self.expired_error());
        };
        if entry.expired(now, self.lifetime) {
            book.entries.remove(&id);
            return Err(self.expired_error());
        }
        if entry.binding.plane != plane {
            return Err(PublicError::invalid_input(
                "client.review_plane_mismatch",
                "This review belongs to another Plane.",
            ));
        }
        if let Some((result, _)) = &entry.result {
            return Ok((entry.binding, Attempt::Known(result.clone())));
        }
        let (binding, scope) = (entry.binding, entry.scope);
        if book.entries.iter().any(|(other, entry)| {
            *other != id
                && entry.binding.plane == binding.plane
                && entry.scope == scope
                && entry.unresolved()
        }) {
            return Err(self.unresolved_error());
        }
        let entry = book
            .entries
            .get_mut(&id)
            .ok_or_else(PublicError::internal)?;
        check(&mut entry.value)?;
        let value = entry.value.clone();
        if entry.attempted {
            Ok((binding, Attempt::Retry(value)))
        } else {
            entry.attempted = true;
            Ok((binding, Attempt::Fresh(value)))
        }
    }

    /// Records the definite outcome of review `id`. It is replayed to later
    /// attempts for `lifetime`, so a lost IPC reply is never resent.
    pub(crate) fn record(&self, id: Uuid, result: R) {
        let now = (self.now)();
        if let Ok(mut book) = self.inner.lock() {
            if let Some(entry) = book.entries.get_mut(&id) {
                entry.attempted = true;
                entry.result = Some((result, now));
            }
        }
    }

    /// Drops every review of a Plane, for example after its store was
    /// replaced by an older snapshot.
    pub(crate) fn clear_plane(&self, plane: PlaneId) {
        if let Ok(mut book) = self.inner.lock() {
            book.entries.retain(|_, entry| entry.binding.plane != plane);
        }
    }

    /// Drops every review of a Plane except `keep`, for example the review
    /// that replaced the Plane's store and has yet to record its outcome.
    pub(crate) fn clear_plane_except(&self, plane: PlaneId, keep: Uuid) {
        if let Ok(mut book) = self.inner.lock() {
            book.entries
                .retain(|id, entry| *id == keep || entry.binding.plane != plane);
        }
    }

    /// The attempted, unresolved review of `scope` on a Plane, if any: its
    /// ID, the binding it was issued against and its value.
    pub(crate) fn unresolved(
        &self,
        plane: PlaneId,
        scope: Uuid,
    ) -> Result<Option<(Uuid, PlaneBinding, T)>, PublicError> {
        let book = self.inner.lock().map_err(|_| PublicError::internal())?;
        Ok(book
            .entries
            .iter()
            .find(|(_, entry)| {
                entry.binding.plane == plane && entry.scope == scope && entry.unresolved()
            })
            .map(|(id, entry)| (*id, entry.binding, entry.value.clone())))
    }

    /// Drops one review, for example an unresolved one whose Plane is now a
    /// different Plane and so can never be resent.
    pub(crate) fn forget(&self, id: Uuid) {
        if let Ok(mut book) = self.inner.lock() {
            book.entries.remove(&id);
        }
    }

    fn expired_error(&self) -> PublicError {
        PublicError::invalid_input(
            self.codes.review_expired,
            "This review expired. Review the request again.",
        )
    }

    fn unresolved_error(&self) -> PublicError {
        PublicError::conflict(
            self.codes.request_unresolved,
            "Jet is still confirming an earlier request.",
        )
    }
}

fn oldest<T, R>(
    entries: &HashMap<Uuid, Entry<T, R>>,
    eligible: impl Fn(&Entry<T, R>) -> bool,
) -> Option<Uuid> {
    entries
        .iter()
        .filter(|(_, entry)| eligible(entry))
        .min_by_key(|(_, entry)| entry.order)
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use uuid::Uuid;

    use super::{
        Attempt, Ledger, RECOVERY_CODES, RETENTION_CODES, REVIEW_CAPACITY, REVIEW_LIFETIME,
    };
    use crate::jet::{
        errors::PublicError,
        planes::{PlaneBinding, PlaneId},
    };

    const LIFETIME: Duration = Duration::from_secs(600);

    struct Clock(Arc<Mutex<Instant>>);

    impl Clock {
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    fn ledger(capacity: usize) -> (Ledger<&'static str, u32>, Clock) {
        let time = Arc::new(Mutex::new(Instant::now()));
        let reader = Arc::clone(&time);
        (
            Ledger::with_clock(
                RETENTION_CODES,
                LIFETIME,
                capacity,
                Arc::new(move || *reader.lock().unwrap()),
            ),
            Clock(time),
        )
    }

    fn local() -> PlaneBinding {
        PlaneBinding {
            plane: PlaneId::Local,
            identity: None,
        }
    }

    fn remote() -> PlaneBinding {
        PlaneBinding {
            plane: PlaneId::Remote(Uuid::from_u128(9)),
            identity: Some(Uuid::from_u128(10)),
        }
    }

    fn scope(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn pass(_: &mut &'static str) -> Result<(), PublicError> {
        Ok(())
    }

    fn code<T: std::fmt::Debug>(result: Result<T, PublicError>) -> String {
        result.unwrap_err().code
    }

    #[test]
    fn an_unattempted_review_expires_after_its_lifetime() {
        let (ledger, clock) = ledger(8);
        let id = ledger.issue(local(), scope(1), "forget").unwrap();
        clock.advance(LIFETIME + Duration::from_secs(1));
        assert_eq!(
            code(ledger.attempt(id, PlaneId::Local, pass)),
            "retention.review_expired"
        );
        // An unknown ID is the same.
        assert_eq!(
            code(ledger.attempt(Uuid::from_u128(77), PlaneId::Local, pass)),
            "retention.review_expired"
        );
    }

    #[test]
    fn an_attempted_review_never_expires_and_retries_unchanged() {
        let (ledger, clock) = ledger(8);
        let id = ledger.issue(local(), scope(1), "forget").unwrap();
        assert_eq!(
            ledger.attempt(id, PlaneId::Local, pass).unwrap(),
            (local(), Attempt::Fresh("forget"))
        );
        clock.advance(LIFETIME * 10);
        assert_eq!(
            ledger.attempt(id, PlaneId::Local, pass).unwrap().1,
            Attempt::Retry("forget")
        );
    }

    #[test]
    fn a_recorded_outcome_replays_then_ages_out() {
        let (ledger, clock) = ledger(8);
        let id = ledger.issue(local(), scope(1), "forget").unwrap();
        ledger.attempt(id, PlaneId::Local, pass).unwrap();
        ledger.record(id, 7);
        clock.advance(LIFETIME);
        assert_eq!(
            ledger.attempt(id, PlaneId::Local, pass).unwrap().1,
            Attempt::Known(7)
        );
        clock.advance(Duration::from_secs(1));
        assert_eq!(
            code(ledger.attempt(id, PlaneId::Local, pass)),
            "retention.review_expired"
        );
    }

    #[test]
    fn a_check_failure_leaves_the_review_unattempted() {
        let (ledger, _) = ledger(8);
        let id = ledger.issue(local(), scope(1), "forget").unwrap();
        let refused = ledger.attempt(id, PlaneId::Local, |_| {
            Err(PublicError::invalid_input(
                "retention.stop_unacknowledged",
                "m",
            ))
        });
        assert_eq!(code(refused), "retention.stop_unacknowledged");
        assert_eq!(
            ledger.attempt(id, PlaneId::Local, pass).unwrap().1,
            Attempt::Fresh("forget")
        );
    }

    #[test]
    fn the_check_can_lock_the_stored_value() {
        let (ledger, _) = ledger(8);
        let id = ledger.issue(local(), scope(1), "unset").unwrap();
        ledger
            .attempt(id, PlaneId::Local, |value| {
                *value = "forget";
                Ok(())
            })
            .unwrap();
        let switched = ledger.attempt(id, PlaneId::Local, |value| {
            if *value == "delete_everywhere" {
                Ok(())
            } else {
                Err(PublicError::conflict("retention.mode_locked", "m"))
            }
        });
        assert_eq!(code(switched), "retention.mode_locked");
    }

    #[test]
    fn eviction_takes_resolved_reviews_then_the_oldest_unattempted() {
        let (ledger, _) = ledger(3);
        let resolved = ledger.issue(local(), scope(1), "a").unwrap();
        ledger.attempt(resolved, PlaneId::Local, pass).unwrap();
        ledger.record(resolved, 1);
        let old = ledger.issue(local(), scope(2), "b").unwrap();
        let pending = ledger.issue(local(), scope(3), "c").unwrap();
        ledger.attempt(pending, PlaneId::Local, pass).unwrap();

        let fourth = ledger.issue(local(), scope(4), "d").unwrap();
        assert_eq!(
            code(ledger.attempt(resolved, PlaneId::Local, pass)),
            "retention.review_expired"
        );
        assert!(ledger.attempt(old, PlaneId::Local, pass).is_ok());

        // `old` is attempted now; the only unattempted review left is `fourth`.
        let fifth = ledger.issue(local(), scope(5), "e").unwrap();
        assert_eq!(
            code(ledger.attempt(fourth, PlaneId::Local, pass)),
            "retention.review_expired"
        );
        assert!(ledger.attempt(fifth, PlaneId::Local, pass).is_ok());

        // Three attempted, unresolved reviews: nothing can be evicted.
        assert_eq!(
            code(ledger.issue(local(), scope(6), "f")),
            "client.review_limit"
        );
        assert_eq!(
            ledger.attempt(pending, PlaneId::Local, pass).unwrap().1,
            Attempt::Retry("c")
        );
    }

    #[test]
    fn an_unresolved_request_blocks_its_scope_on_that_plane_only() {
        let (ledger, _) = ledger(8);
        let first = ledger.issue(local(), scope(1), "forget").unwrap();
        let second = ledger.issue(local(), scope(1), "forget").unwrap();
        ledger.attempt(first, PlaneId::Local, pass).unwrap();

        assert_eq!(
            code(ledger.issue(local(), scope(1), "again")),
            "retention.request_unresolved"
        );
        assert_eq!(
            code(ledger.attempt(second, PlaneId::Local, pass)),
            "retention.request_unresolved"
        );
        // The same Conversation UUID on another Plane is another scope.
        assert!(ledger.issue(remote(), scope(1), "forget").is_ok());
        assert!(ledger.issue(local(), scope(2), "forget").is_ok());

        ledger.record(first, 3);
        assert!(ledger.issue(local(), scope(1), "again").is_ok());
        assert_eq!(
            ledger.attempt(second, PlaneId::Local, pass).unwrap().1,
            Attempt::Fresh("forget")
        );
    }

    #[test]
    fn a_review_from_one_plane_is_refused_on_another() {
        let (ledger, _) = ledger(8);
        let id = ledger.issue(remote(), scope(1), "forget").unwrap();
        assert_eq!(
            code(ledger.attempt(id, PlaneId::Local, pass)),
            "client.review_plane_mismatch"
        );
        assert_eq!(
            ledger.attempt(id, remote().plane, pass).unwrap(),
            (remote(), Attempt::Fresh("forget"))
        );
    }

    #[test]
    fn clearing_a_plane_drops_only_its_reviews() {
        let (ledger, _) = ledger(8);
        let local_review = ledger.issue(local(), scope(1), "a").unwrap();
        let remote_review = ledger.issue(remote(), scope(1), "b").unwrap();
        ledger.clear_plane(PlaneId::Local);
        assert_eq!(
            code(ledger.attempt(local_review, PlaneId::Local, pass)),
            "retention.review_expired"
        );
        assert!(ledger.attempt(remote_review, remote().plane, pass).is_ok());
    }

    #[test]
    fn clearing_a_plane_can_keep_one_review() {
        let (ledger, _) = ledger(8);
        let kept = ledger.issue(local(), scope(1), "kept").unwrap();
        let other = ledger.issue(local(), scope(2), "other").unwrap();
        ledger.attempt(other, PlaneId::Local, pass).unwrap();
        let remote_review = ledger.issue(remote(), scope(2), "b").unwrap();
        ledger.clear_plane_except(PlaneId::Local, kept);
        assert!(ledger.attempt(kept, PlaneId::Local, pass).is_ok());
        assert_eq!(
            code(ledger.attempt(other, PlaneId::Local, pass)),
            "retention.review_expired"
        );
        // The dropped unresolved review no longer blocks its scope.
        assert!(ledger.issue(local(), scope(2), "again").is_ok());
        assert!(ledger.attempt(remote_review, remote().plane, pass).is_ok());
    }

    #[test]
    fn an_unresolved_review_is_found_by_plane_and_scope_until_it_resolves() {
        let (ledger, _) = ledger(8);
        let id = ledger.issue(remote(), scope(1), "epoch").unwrap();
        // Unattempted is not unresolved.
        assert_eq!(ledger.unresolved(remote().plane, scope(1)).unwrap(), None);
        ledger.attempt(id, remote().plane, pass).unwrap();
        assert_eq!(
            ledger.unresolved(remote().plane, scope(1)).unwrap(),
            Some((id, remote(), "epoch"))
        );
        assert_eq!(ledger.unresolved(remote().plane, scope(2)).unwrap(), None);
        assert_eq!(ledger.unresolved(PlaneId::Local, scope(1)).unwrap(), None);
        ledger.record(id, 1);
        assert_eq!(ledger.unresolved(remote().plane, scope(1)).unwrap(), None);

        let stuck = ledger.issue(remote(), scope(1), "stuck").unwrap();
        ledger.attempt(stuck, remote().plane, pass).unwrap();
        ledger.forget(stuck);
        assert_eq!(ledger.unresolved(remote().plane, scope(1)).unwrap(), None);
        assert!(ledger.issue(remote(), scope(1), "fresh").is_ok());
    }

    #[test]
    fn the_default_ledger_uses_its_namespace_the_preview_lifetime_and_capacity() {
        assert_eq!(REVIEW_LIFETIME, Duration::from_secs(600));
        assert_eq!(REVIEW_CAPACITY, 256);
        let ledger: Ledger<(), ()> = Ledger::new(RECOVERY_CODES);
        assert_eq!(ledger.lifetime, REVIEW_LIFETIME);
        assert_eq!(ledger.capacity, REVIEW_CAPACITY);
        assert_eq!(
            code(ledger.attempt(Uuid::from_u128(1), PlaneId::Local, |_| Ok(()))),
            "recovery.review_expired"
        );
        let first = ledger.issue(local(), scope(1), ()).unwrap();
        ledger.attempt(first, PlaneId::Local, |_| Ok(())).unwrap();
        assert_eq!(
            code(ledger.issue(local(), scope(1), ())),
            "recovery.request_unresolved"
        );
    }
}
