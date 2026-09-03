# Store core state under Jet home

Each Plane stores all Jet-core-owned files under the current user's `~/.jet` directory. `jetd` persists identity mappings, native event envelopes, presentation blocks, commands, approval audit, and artifact references in a SQLite WAL store; large artifacts use content-addressed files, and `jetfueld` retains a bounded on-disk spool until `jetd` acknowledges the events. Secret material remains in the platform credential store, with only opaque references under `~/.jet`.
