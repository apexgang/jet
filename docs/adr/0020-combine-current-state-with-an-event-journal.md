# Combine current state with an Event journal

`jetd` stores normalized current state alongside an append-only Event journal instead of reconstructing everything through full event sourcing or retaining only mutable rows. Entities use globally unique UUIDv7 identifiers, streams use monotonic sequence numbers, commands are idempotent by ID, and transactional snapshots permit safe compaction while preserving required audit history; representative append, replay, projection, and query paths are guarded by Rust benchmarks.
