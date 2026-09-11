# Store Recovery

Issue #49 implements ADR-0097, ADR-0073, and ADR-0077 through Jet
protocol minor 37. A Recovery
snapshot is a complete copy of the Plane store that SQLite wrote with
`VACUUM INTO`, that passed a full `PRAGMA integrity_check` on its own
connection, and that was then given its final name durably: owner-only,
synced, renamed over from a `.pending-` file, with the directory synced
after it. A file under a snapshot name is therefore always restorable;
a crash leaves at most a pending file, which the next rotation removes.

Snapshots live under `~/.jet/snapshots/` as
`plane-<unix_ms>-<reason>.sqlite3`. The reason is `daily`,
`maintenance`, or `migration-<version>`, where the version is the newest
migration the store had applied when the copy was taken. Disk-pressure collection never visits the directory:
it only removes content-addressed payloads and pending uploads under
`artifacts/payloads` and `cache` (see [Disk pressure](disk-pressure.md)).

## When a snapshot is taken

- **Before a schema migration.** `Store::open` reads which migrations
  the store has applied. When this build knows one it has not, the store
  is copied and verified before anything runs. A store with no schema
  tracker yet is new and is not copied. A failed copy fails the open,
  because ADR-0073 requires the rollback point to exist before the
  migration. A failed migration leaves the store at its previous version
  with the snapshot beside it.
- **After the first meaningful change of a day.** The store remembers
  whether any write transaction committed through it since the newest
  snapshot, and which UTC day that snapshot belongs to. A daemon start,
  which bypasses the write path, is not a change, and what an earlier
  daemon did after its last snapshot is unknown and counts as nothing:
  it is copied with the next change, or before the next maintenance. The
  daemon's maintenance loop, which every Command and Effect commit wakes,
  asks for the day's snapshot and takes it when one is due. A failure is
  reported on stderr and retried on the next wake; the change it
  followed is already durable.
- **Before destructive maintenance.** The Security-audit retention sweep
  and unreferenced Craft Artifact collection run when the core starts on
  a trusted audit. When no snapshot belongs to that day yet, one is
  taken first, whether or not the store is known to have changed.

Routine and maintenance snapshots share the one-per-day bound, so a
sweep on a day that already has its snapshot runs without a fresh one;
what it removes is older than the retention window by days, and the
snapshot directory is what bounds the cost of the rule. Migration
snapshots are always taken. Snapshot stamps come from the core clock,
except the pre-migration one, which the store stamps from the wall clock
before any clock-carrying caller exists.

## Retention

Rotation runs after every snapshot and keeps, by count rather than age:

| Tier | Kept |
|------|------|
| Daily | The newest snapshot of each of the seven newest days that have one |
| Weekly | Among older snapshots, the newest of each of the four newest weeks |
| Rollback | The newest `migration` snapshot of each of the two newest schema versions, whatever its age |

Days and weeks are computed in UTC from the stamp. Everything else is
removed, along with any `.pending-` file. An idle Plane keeps what it
has: nothing expires by age alone. Repeated attempts at one migration,
each preceded by a copy of the same unchanged store, share a version and
keep only their newest copy, so they cannot crowd out the rollback point
of the release before.

The Security audit head beside the database is not copied. Restoring a
snapshot moves authoritative state backwards, and the head left in place
is what makes that visible to the audit, which an owner then carries on
from by beginning a new epoch (ADR-0105).

## Read-only Recovery mode

Every open of the store runs SQLite's `PRAGMA quick_check` before any
migration. Damage met on the way to the check counts as the check
failing. The deeper `integrity_check` runs on every snapshot as it is
taken; scheduling it over the live store while idle is left to a
follow-up. A store that fails it, or whose migration fails, still opens,
and `jetd` still serves it: the ready line carries `"recovery":
"read_only"` and the status Query reports

```json
"recovery": {
  "state": "read_only",
  "reason": "integrity_check_failed",
  "snapshots": [
    {"name": "plane-1757000000000-daily.sqlite3",
     "taken_at_unix_ms": 1757000000000, "reason": "daily", "bytes": 405504}
  ]
}
```

with `reason` either `integrity_check_failed` or `migration_failed`, and
the verified snapshots newest first. A serving Plane reports `"state":
"serving"` and the same list. SQLite's own account of the damage stays
local, on `jetd`'s stderr and in the store's integrity state; it never
crosses the wire (ADR-0061).

In Recovery mode Queries are answered as far as the damage allows, so
the Plane can be diagnosed and, once #50 lands, exported. Every Command
but one is refused with `recovery.read_only`, in the `unavailable`
category and retryable, before any receipt exists: the same Command
identity succeeds after restoration. Nothing that writes runs: the
daemon start is not recorded, the audit retention sweep, Artifact
collection, and search indexing are skipped, no worker reconciles or
dispatches, and surviving `jetfueld` executions are not reconnected. The
Security audit is still validated when the store can answer; when it
cannot, `security` is absent from the status.

The one Command is `restore_recovery_snapshot`, naming a snapshot from
the list. It runs beside the receipt pipeline, because the store a
receipt would go to is the one being replaced, so it is not idempotent:
a second request meets `recovery.not_read_only`, as does any request on
a serving Plane. Restoration closes the store's connection, moves
`plane.sqlite3` and its journal files aside under a name that says what
the check found them to be, `plane.sqlite3.damaged-<unix_ms>` after a
failed integrity check or `plane.sqlite3.unmigrated-<unix_ms>` after a
failed migration, never deleting them, copies the snapshot into place,
and reopens it through the same checks as any open, migrating it when the
snapshot predates this release. A restored store that fails its own
checks leaves the Plane in Recovery mode with `recovery.restore_failed`.
On success the daemon start is recorded, the Security audit is validated
against the restored state, the search index catches up, and the workers
that were waiting run the start-time reconciliation, including `jetfueld`
reconnection, before serving as usual. The reply names the snapshot and
the file that was moved aside:

```json
{"type": "recovery_snapshot_restored",
 "snapshot": "plane-1757000000000-daily.sqlite3",
 "replaced": "plane.sqlite3.damaged-1757000100000"}
```

After a failed migration the store is intact at its previous version, and
the release before this one opens it as it is (ADR-0073); restoring a
snapshot with the same release runs the same migration again, and only
helps when the failure came from damaged content the snapshot predates.

The Security audit head is not restored with the snapshot, and the
restoration is recorded in the audit as `recovery.snapshot_restored`,
with destructive risk, only when the restored chain still reaches the
head. A snapshot older than the newest audit record leaves the head
naming a record the store no longer holds; appending to the chain would
publish a new head over that, so nothing is appended, the next
validation reports `head_not_in_store`, and the Plane is
Security-degraded until an owner begins a new audit epoch, whose first
record says so. That degradation survives restarts; it is the record of
the rollback the audit itself keeps (ADR-0105). The Deletion ledger of ADR-0102 arrives
with #53; until then a restoration can bring back a Conversation deleted
after the snapshot was taken, which is why deleted content is bounded by
the retention above.

## Recovery mode is not the other degradations

Security-degraded mode (ADR-0105) keeps serving Runs and every ordinary
Command; only trust, policy, and destructive changes wait. Disk pressure
(ADR-0079) refuses admissions one at a time and is recomputed on each.
Orphaned executions are `jetfueld` processes whose Run cannot be matched
to Plane state, resolved through `resolve_execution`. Recovery mode is
the store itself being in doubt: it blocks every mutation, and its way
out replaces the store.

## Validation

`just test -p jet-store snapshot` covers naming, verification,
publication, the once-per-day rule, and retention tiers;
`just test -p jet-store recovery` covers a damaged store opening
read-only, a failed migration, and restoration.
`just test -p jet-core store_recovery` covers the daily and maintenance
moments, refusal, restoration, and the audit through the core.
`just test -p jet-daemon --test store_recovery`, run from `packages/`,
drives a real daemon through a snapshot, real damage, read-only service,
restoration, and a clean restart.
