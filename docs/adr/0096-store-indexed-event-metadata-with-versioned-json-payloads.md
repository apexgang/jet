# Store indexed Event metadata with versioned JSON payloads

Event-journal rows use typed indexed columns for identity, Actor, Plane sequence, time, and kind, plus a bounded versioned JSON payload for event-specific data. Raw terminal bytes and large Harness-native payloads remain in bounded replay storage or content-addressed Artifacts referenced by Events. This reuses the protocol's JSON tooling and keeps durable state portable without making schema evolution depend on Rust-only binary serialization.
