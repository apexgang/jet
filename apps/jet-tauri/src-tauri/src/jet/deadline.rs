//! How long the shell waits for a Plane (Wave 4, D6). A stopped `jetd` or a
//! silently dropped remote link accepts a request and never answers; without
//! a deadline a feed stays "Connected" and a request stays pending forever.
//!
//! | Wait | Deadline | On expiry |
//! | --- | --- | --- |
//! | Liveness: local connect and handshake, `PlaneClient::status`, the status read that ends a remote login, one event-feed page | 30 s | `transport.offline` (retryable): a feed reports Reconnecting and dials again; a remote login backs off |
//! | Query: any other read on an open connection (`Connection::query`) | 90 s | `transport.offline` (retryable) |
//! | Command (`Connection::command`, and the Commands `PlaneClient` sends itself) | 180 s | `command.outcome_unknown`: the Command ID is kept, so a retry resends the same request (ADR-0093) |
//!
//! The values are conservative. A healthy local `jetd` answers liveness
//! reads in milliseconds and a remote one within a few round trips, so 30 s
//! is reached only by a hung peer. Some Queries make the Plane reach the
//! network itself, each request bounded at 30 s by the core: an Extension
//! catalog is one such step, and Craft discovery is two in sequence (the
//! release metadata, then the specification), up to 60 s before the answer
//! crosses ssh. Queries get 90 s, so a slow but healthy discovery ends with
//! the core's own answer rather than the shell's `transport.offline`. Some
//! Commands do their work before they answer (restoring a Recovery
//! snapshot, preparing a Craft installation, which discovers the release
//! again and downloads its artifact), so Commands get twice a Query's
//! deadline.
//!
//! Not covered here: the signed remote handshake and the pairing exchange,
//! which `jet-client` bounds itself (15 s), and the keyring unlock before a
//! remote login, whose prompt has its own limit in `keystore.rs` because a
//! person may take a while to answer it. An attached terminal (its output,
//! input and resizes) and the feed's poll interval are streams, not
//! requests, and have no deadline; only the attach reply is bounded.
//!
//! An expired wait is reported as `ClientError::Io(TimedOut)` carrying
//! `Expired`, so every existing path already treats it as a lost transport:
//! never a definite refusal, and never a reason to drop a Command ID. The
//! connection it happened on then fails every later request at once
//! (`Connection::bounded`), local or remote, so a flow of many reads waits
//! out one deadline rather than one per read.

use std::{fmt, future::Future, io, pin::Pin, sync::Arc, time::Duration};

use jet_client::ClientError;

/// A local connect and handshake, a status read, or one event-feed page.
pub(crate) const LIVENESS_DEADLINE: Duration = Duration::from_secs(30);
/// One Query, including reads the Plane answers from the network.
pub(crate) const QUERY_DEADLINE: Duration = Duration::from_secs(90);
/// One Command, from sending it to its durable answer.
pub(crate) const COMMAND_DEADLINE: Duration = Duration::from_secs(180);

/// What the shell is waiting for, which decides the deadline and what its
/// expiry means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Work a healthy Plane does at once: an expiry is a lost transport.
    Liveness,
    /// Reads only: an expiry is a lost transport.
    Query,
    /// A durable change: an expiry leaves the outcome unknown.
    Command,
}

impl Wait {
    pub(crate) const fn limit(self) -> Duration {
        match self {
            Self::Liveness => LIVENESS_DEADLINE,
            Self::Query => QUERY_DEADLINE,
            Self::Command => COMMAND_DEADLINE,
        }
    }
}

pub(crate) type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The clock deadlines run on. Production uses tokio's timer; tests advance
/// a manual clock instead of waiting.
pub(crate) trait Timer: Send + Sync {
    /// Completes once `limit` has passed, measured from this call.
    fn sleep(&self, limit: Duration) -> Sleep;
}

struct TokioTimer;

impl Timer for TokioTimer {
    fn sleep(&self, limit: Duration) -> Sleep {
        Box::pin(tokio::time::sleep(limit))
    }
}

/// The deadline policy shared by every client of one registry.
#[derive(Clone)]
pub(crate) struct Deadlines {
    timer: Arc<dyn Timer>,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            timer: Arc::new(TokioTimer),
        }
    }
}

impl fmt::Debug for Deadlines {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Deadlines { .. }")
    }
}

impl Deadlines {
    #[cfg(test)]
    pub(crate) fn on(timer: Arc<dyn Timer>) -> Self {
        Self { timer }
    }

    /// `request`, or `Expired(wait)` once its deadline passes. The deadline
    /// starts now, not at the first poll. The request is dropped at expiry;
    /// a Command may still complete on the Plane.
    pub(crate) fn bound<T>(
        &self,
        wait: Wait,
        request: impl Future<Output = Result<T, ClientError>>,
    ) -> impl Future<Output = Result<T, Box<ClientError>>> {
        let deadline = self.timer.sleep(wait.limit());
        async move {
            tokio::select! {
                biased;
                result = request => result.map_err(Box::new),
                () = deadline => Err(Box::new(expired(wait))),
            }
        }
    }
}

/// The marker inside an expired wait's `io::Error`.
#[derive(Debug, Clone, Copy)]
struct Expired(Wait);

impl fmt::Display for Expired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Wait::Liveness => formatter.write_str("the Plane did not answer in time"),
            Wait::Query => formatter.write_str("the Plane did not answer the Query in time"),
            Wait::Command => formatter.write_str("the Plane did not answer the Command in time"),
        }
    }
}

impl std::error::Error for Expired {}

fn expired(wait: Wait) -> ClientError {
    ClientError::Io(io::Error::new(io::ErrorKind::TimedOut, Expired(wait)))
}

/// The wait whose deadline `error` reports, or `None` for any other error.
pub(crate) fn expired_wait(error: &ClientError) -> Option<Wait> {
    match error {
        ClientError::Io(error) => error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<Expired>())
            .map(|expired| expired.0),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod manual {
    use std::time::Duration;

    use tokio::sync::watch;

    use super::{Sleep, Timer};

    /// A clock that moves only when a test advances it.
    pub(crate) struct ManualTimer {
        now: watch::Sender<Duration>,
    }

    impl Default for ManualTimer {
        fn default() -> Self {
            Self {
                now: watch::Sender::new(Duration::ZERO),
            }
        }
    }

    impl ManualTimer {
        pub(crate) fn advance(&self, by: Duration) {
            self.now.send_modify(|now| *now += by);
        }
    }

    impl Timer for ManualTimer {
        fn sleep(&self, limit: Duration) -> Sleep {
            let mut now = self.now.subscribe();
            let until = *now.borrow() + limit;
            Box::pin(async move {
                if now.wait_for(|now| *now >= until).await.is_err() {
                    // The clock is gone: nothing can expire any more.
                    std::future::pending::<()>().await;
                }
            })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{future::Future, pin::Pin, sync::Arc, task::Poll, time::Duration};

    use jet_client::ClientError;

    use super::{expired_wait, manual::ManualTimer, Deadlines, Wait, COMMAND_DEADLINE};

    /// Polls `future` once: whether it is still waiting.
    pub(crate) async fn still_pending<F: Future + Unpin>(future: &mut F) -> bool {
        std::future::poll_fn(|context| {
            Poll::Ready(Pin::new(&mut *future).poll(context).is_pending())
        })
        .await
    }

    /// The core's bound on one network request it makes for a Query
    /// (`jet-core` `craft/repository.rs`, `extension/work.rs`).
    const CORE_NETWORK_STEP: Duration = Duration::from_secs(30);

    #[test]
    fn deadlines_are_conservative_and_commands_wait_longest() {
        assert_eq!(Wait::Liveness.limit(), Duration::from_secs(30));
        assert_eq!(Wait::Query.limit(), Duration::from_secs(90));
        assert_eq!(Wait::Command.limit(), Duration::from_secs(180));
        // Craft discovery makes two network requests in sequence, each
        // bounded by the core: a slow but healthy one must not expire in
        // the shell (and on a remote Plane drop the shared ssh session).
        assert!(Wait::Query.limit() > 2 * CORE_NETWORK_STEP);
        // Installing a Craft discovers it again, then downloads it.
        assert!(Wait::Command.limit() > 3 * CORE_NETWORK_STEP);
        assert!(Wait::Command.limit() > Wait::Query.limit());
    }

    #[tokio::test]
    async fn a_request_ends_at_its_deadline_and_not_before() {
        let timer = Arc::new(ManualTimer::default());
        let deadlines = Deadlines::on(timer.clone());
        let (answer, answered) = tokio::sync::oneshot::channel::<u8>();

        // An answer just inside the deadline is kept.
        let request = deadlines.bound(Wait::Command, async {
            answered.await.map_err(|_| ClientError::Closed)
        });
        timer.advance(COMMAND_DEADLINE - Duration::from_millis(1));
        answer.send(7).unwrap();
        assert_eq!(request.await.unwrap(), 7);

        // A request that never answers expires at its deadline: a Query
        // after 90 s, even though a Command would still be waiting.
        let (_keep, never) = tokio::sync::oneshot::channel::<u8>();
        let (_keep_command, never_command) = tokio::sync::oneshot::channel::<u8>();
        let request = deadlines.bound(Wait::Query, async {
            never.await.map_err(|_| ClientError::Closed)
        });
        let command = deadlines.bound(Wait::Command, async {
            never_command.await.map_err(|_| ClientError::Closed)
        });
        tokio::pin!(command);
        timer.advance(Wait::Query.limit());
        let error = request.await.unwrap_err();
        assert!(
            still_pending(&mut command).await,
            "a Command outlives the Query deadline"
        );
        timer.advance(COMMAND_DEADLINE - Wait::Query.limit());
        assert_eq!(
            expired_wait(&command.await.unwrap_err()),
            Some(Wait::Command)
        );
        assert_eq!(expired_wait(&error), Some(Wait::Query));
        assert!(
            matches!(&*error, ClientError::Io(io) if io.kind() == std::io::ErrorKind::TimedOut)
        );
        assert_eq!(expired_wait(&ClientError::Closed), None);
    }
}
