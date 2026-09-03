# Commit Effects through a transactional outbox

One SQLite transaction records accepted domain Events, current-state projections, and pending external Effects. Workers perform those Effects only after commit and record observable outcomes afterward; unresolved entries survive restart. This Effect outbox prevents an acknowledged Command from disappearing between durable state and process, Git, filesystem, or network work without pretending an external side effect can be atomically committed with SQLite.
