# Expose Command, Query, and Event Interfaces from core

`jet-core` presents three conceptual entry points: execute an authenticated Command, run a Query returning a snapshot or page, and subscribe from an Event-journal cursor. Its Interface includes ordering, idempotency, authorization, error, and performance guarantees while hiding scheduling, transactions, retries, cleanup, approvals, and lifecycle transitions. `jetd` is primarily a transport Adapter around this deep Module rather than a second home for orchestration logic.
