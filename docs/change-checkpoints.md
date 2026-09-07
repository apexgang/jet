# Change checkpoints

Issue #25 implements ADR-0030 and ADR-0084 at the Core and client protocol
boundaries. A Run's initial working tree is captured durably before its Harness
starts. Each completed or interrupted turn retains the checked-out commits,
working-tree objects, uncommitted patches, changed-file modes and object names,
and SHA-256 patch Artifact references. Plane, Conversation, Run, and optional
Workspace identities are retained; a Local-checkout Run has no Workspace identity.

Capture uses a scratch Git index. It creates no commits and leaves the user's
index and branches alone. Internal refs keep captured trees and rewritten commits
reachable across Git garbage collection. Current diff reads do not pin extra refs.
Git ignore, symlink, submodule, and nested-repository behavior follows the existing
Workspace capture rules. Eligible regular files larger than 512 MiB are excluded
before staging. Their paths, observed sizes, and modes remain in checkpoint
metadata, with absent content identities and external/unknown origin. Snapshot
`content_complete` tells clients whether the Git tree omitted live file content. Renames are represented as a deletion and an addition.

Patches use Git's full-index binary patch format, with external diff and textconv
programs disabled. They stream into owner-only temporary files, are capped at
512 MiB each, and are synced and atomically published before SQLite references
commit. A descriptor lock serializes hash deduplication and publication. Newly
ingested bytes are reserved against a durable 2 GiB per-Run budget before
publication; repeated hashes consume no extra budget. Reservations survive
restart, including a conservative charge if publication was interrupted. If a
patch exceeds an ingestion limit, its hash and size remain available with an
explicit `run_budget_exceeded` or `artifact_size_exceeded` availability. The
checkpoint and Run still complete; clients must treat this as unavailable content,
not an empty patch. Hash paths and directory handles reject symlink redirection. Other capture
failures are reported rather than substituted with empty diffs or invented origin.

## Queries

Client protocol 1.17 adds `change_diff`, `next_change_diff`, and `change_artifact`. Older peers are
refused these requests before execution. `jet-client` provides `change_diff`,
`next_change_diff`, and `change_artifact` methods.

- `current`: the Run's initial working tree compared with Git now.
- `turn`: one completed or interrupted turn, numbered from one.
- `final`: the Run's initial working tree compared with its final captured turn;
  rejected until the Run is terminal.
- `historical`: two retained boundary numbers, where zero is the Run baseline.

Diff replies include an Event cursor, complete patch Artifact address, at most
128 KiB of patch preview, and a bounded page of file metadata. Follow
`next_page` with `next_change_diff` to inspect every path. Opaque keyset cursors
bind the Run, scope, Git snapshots, and file metadata for five minutes, retaining
the original Event fence. Unrelated Plane Events do not invalidate them. Changed
assumptions, expired cursors, or a daemon restart return `pagination.stale` with
restart metadata; Core never combines pages from different snapshots. Artifact reads return at most 64 KiB per call;
consumers verify the assembled bytes against the advertised SHA-256.

## Evidence and turn integration

Craft 1.3 adds `file_changed`, `turn_started`, and `turn_ended`. The daemon derives
Harness origin from the connection's Run identity. A Craft cannot select user or
terminal origin. File evidence includes a native activity identity, relative path,
and the exact before/after Git objects and modes.

Trusted in-process User-edit and Workspace-terminal Adapters use
`Core::record_change_evidence` with identities derived from their authenticated
Command or owned terminal operation. These are integration hooks for those
workflows; this issue does not introduce a file editor or terminal UI. The caller
must supply an observed operation receipt, not a filesystem notification.

Evidence is durable before turn completion and retained with the immutable
checkpoint. Attribution requires a complete content/mode chain. Gaps,
contradictions, uncorrelated edits, and external overwrites remain external or
unknown; complete chains from several sources are mixed. Conflicting receipts or
more than 256 receipts mark the turn evidence incomplete without blocking capture.
Aggregate scopes compose verified checkpoint transitions and inspect gaps between
turns, preserving unknown origins through later changes and content reversions.

The first turn is captured before `Start`. Queued turns capture their next
boundary in the same transaction that claims input, before native delivery.
Matching Craft completion settles the queue and checkpoint together. A later
Craft start notification preserves that earlier boundary. For autonomous turns,
a Craft holds
native input until the host acknowledges the source record containing
`turn_started`. After `turn_ended`, it waits for that boundary's acknowledgement
before admitting further work. Legacy `completed` closes an active turn; process
exit or proven loss closes an unfinished turn as interrupted. Checkpoints commit
with source progress and replay-prefix receipts, so reconnect does not duplicate
a captured turn. No-Visa multi-Plane orchestration can carry these retained
identities; this issue adds no new No-Visa execution path.

## Review stages

The implementation spans Core, storage, Craft, daemon translation, and client
contracts. Its smallest coherent foundation is the Git capture and Artifact
publication layer. The dependent review stages are immutable persistence and
turn recovery, evidence attribution and Craft receipts, then client queries and
pagination. Keeping these seams separate in modules makes the cross-crate change
reviewable while the issue's acceptance checks exercise the complete path.
