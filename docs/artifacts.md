# Artifact publication

General immutable payloads live at `~/.jet/artifacts/payloads/<sha256>`.
SQLite retains only the Run reference, exact size, and a versioned
`artifact.published` Event. Existing Change checkpoint and installed Craft
Artifacts retain their own storage and retention paths.

Protocol minor 31 adds `ArtifactControl` on numbered streams. An upload begins
with `artifact_upload`, a Run identity, and an exact size and SHA-256 declaration.
The daemon grants byte credit. Raw data frames consume that credit and remain
within the negotiated chunk limit, never above 256 KiB. `artifact_commit`
verifies the complete size and SHA-256, synchronizes the private file, publishes
its content address atomically, and commits its Run reference and Event before
replying `artifact_published`. Retrying the same Run/hash returns the same
descriptor without another Event. Existing content is verified before reuse.

A download begins with `artifact_download`. The daemon checks file type and size,
then returns `artifact_downloading` with the size and hash before sending bytes.
Receiver-issued `credit` adds to a byte window, capped at 16 MiB, independently
of the negotiated frame size. Downloads consume it in chunks of at most 64 KiB,
with queued control requests taking priority between chunks. `artifact_finished` follows the
last data frame only after the daemon's streaming hash matches. A client must
also verify size and hash before treating its destination as complete.

Cancellation uses `artifact_cancel` and returns `artifact_canceled`.
Uploads interrupted before commit have no durable reference. An already
requested commit may finish despite cancellation; retry its Run/hash to resolve
the outcome. Up to four transfers, including pending publications, share a
connection. Publication verification runs independently of control requests.
If the connection
is lost after publication, resending the same Run/hash safely resolves the
uncertainty. A crash between filesystem publication and SQLite commit leaves an
unreferenced object. `artifact_collect` removes at most 256 eligible abandoned
files after a 24-hour grace period and returns `artifact_collected` with a count.
Referenced payloads are retained. Collection never follows symbolic links.

The default limits are 512 MiB per Artifact and 2 GiB of newly ingested bytes per
Run, shared with Change checkpoint ingestion. Authenticated Plane Settings
`artifact.max_mib` and `artifact.run_mib` configure these limits. A zero value
disables new nonempty ingestion. Settings take effect at upload admission and
publication. `Core::with_artifact_limits` supplies host defaults. The
free-space reserve defaults to 64 MiB and takes priority over ingestion limits.
New bytes are durably reserved before publication. A crash can conservatively
consume budget without publishing content; it cannot replenish an exhausted
Run's budget. Duplicate stored content costs no new bytes.

Stable failures include `artifact.size_exceeded`, `artifact.invalid_chunk`,
`artifact.size_mismatch`, `artifact.hash_mismatch`, `artifact.run_budget_exceeded`,
`artifact.disk_pressure`, `artifact.not_found`, and `artifact.corrupt`.
Storage details stay in Core diagnostics and do not cross the protocol.

ASVS 2.2.1 and 5.2.1 shape declaration and chunk limits. ASVS 2.3.3 covers
reference/Event transactions. ASVS 5.3.2 and 15.4.2 cover canonical hash names,
pinned directory handles, and publication/collection locking. ASVS 11.4.3 covers
SHA-256 verification. No new third-party dependency is required.
