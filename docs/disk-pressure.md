# Disk pressure

Issue #48 implements ADR-0079. Jet leaves at least 2 GiB or 5% of the
filesystem's total capacity available to its user, whichever is greater.
An embedding application's `ArtifactLimits.free_reserve_bytes` may raise
that floor. It cannot lower it. Each new admission observes available
space again; no timer scans an idle Plane's repositories.

The stable refusal is `storage.disk_pressure`, in the `unavailable`
category. It is transient and does not commit a Command receipt. A retry
with the same identity can succeed after space recovers. Replays of
already committed Commands retain their original outcomes.

New Runs, Workspace creation and forks, Craft installation, and other
operations that can produce large writes must pass admission. Artifact
uploads check their declared size before staging and recheck space for
every chunk and at publication. Checkpoint Artifacts use the same reserve.
Already running executions retain their state and recovery paths. If a
checkpoint payload cannot be ingested, Jet returns `disk_pressure`
availability. Before Git object capture, Jet estimates the changed-file
bytes against both filesystems. Under pressure it reports an explicitly
incomplete HEAD-based snapshot and file metadata without staging content;
that snapshot cannot be used as a fork source or Git delivery boundary.
Recovery does not make an uncaptured boundary complete. Handoff preparation
also returns the stable pressure refusal. Older peers receive the existing
`run_budget_exceeded` metadata-only form. Existing Artifact reads remain
available. Pending launch Effects wait without spending a retry attempt.

`storage.disposable_mib` is a Plane-only Setting, defaulting to 5120 MiB.
Jet protocol 1.36 introduces it. Older peers omit it from snapshots and
cannot query or mutate it by name. Zero prevents new disposable bytes.
Private sparse staging files reserve each upload's declared size across
concurrent Core instances. Publication requires every byte and the verified
hash. Publication, failure, or cancellation releases the reservation;
abandoned files after a crash remain charged until eligible cleanup.
Referenced Artifacts do not consume the disposable budget. Lowering the
budget does not delete them, but publication rechecks the current budget.

The authenticated Artifact collection operation remains available under
pressure. Upload admission also requests a bounded collection pass. A
pass examines at most 64 entries in each of these flat namespaces:

- `artifacts/payloads`: unreferenced SHA-256 payloads and abandoned
  `.pending-<UUID>` files.
- `cache`: disposable SHA-256 cache files and abandoned
  `.pending-<UUID>` files. Cache writers must hold an exclusive file lock
  while an entry is active.

Both require a 24-hour grace period and an available exclusive file lock
before removal. Directory descriptors and no-follow opens prevent link
traversal. Unknown names, nested directories, active uploads, and every
committed Artifact reference are preserved. More than one bounded pass
may be needed for a large backlog.

Cleanup never visits Conversation history, Workspaces, Local checkouts,
Recovery snapshots, or the Deletion ledger. New query-only checkpoint
patches use the disposable cache and its budget.
Legacy checkpoint and Craft Artifacts retain their existing reference and
recovery rules. Reads and Artifact downloads remain available regardless
of the reserve or budget.
External processes can consume disk after a check, so these admission
checks cannot guarantee against an operating-system out-of-space error.
