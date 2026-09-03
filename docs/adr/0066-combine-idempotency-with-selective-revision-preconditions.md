# Combine idempotency with selective revision preconditions

Every Command has an idempotency identity, but only conflict-sensitive mutations require an expected Revision. Append-only user turns enter the authoritative Turn queue without optimistic locking, while renaming, reordering, policy changes, destructive operations, and lifecycle transitions carry preconditions. A conflict returns the current Revision and sufficient safe state for a caller to refresh and retry deliberately.
