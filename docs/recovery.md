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
`plane-<unix_ms>-<reason>.sqlite3`. The reason is `daily`, `migration`,
or `maintenance`. Disk-pressure collection never visits the directory:
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
  whether any write transaction committed since the newest snapshot, and
  which UTC day that snapshot belongs to. A daemon start, which bypasses
  the write path, is not a change. The daemon's maintenance loop, which
  every Command and Effect commit wakes, asks for the day's snapshot and
  takes it when one is due. A failure is reported on stderr and retried
  on the next wake; the change it followed is already durable.
- **Before destructive maintenance.** The Security-audit retention sweep
  and unreferenced Craft Artifact collection run when the core starts on
  a trusted audit. When the store changed since the newest snapshot and
  no snapshot belongs to that day yet, one is taken first.

Routine and maintenance snapshots share the one-per-day rule; migration
snapshots are always taken. Snapshot stamps come from the core clock,
except the pre-migration one, which the store stamps from the wall clock
before any clock-carrying caller exists.

## Retention

Rotation runs after every snapshot and keeps, by count rather than age:

| Tier | Kept |
|------|------|
| Daily | The newest snapshot of each of the seven newest days that have one |
| Weekly | Among older snapshots, the newest of each of the four newest weeks |
| Rollback | The two newest `migration` snapshots, whatever their age |

Days and weeks are computed in UTC from the stamp. Everything else is
removed, along with any `.pending-` file. An idle Plane keeps what it
has: nothing expires by age alone.

The Security audit head beside the database is not copied. Restoring a
snapshot moves authoritative state backwards, and the head left in place
is what makes that visible to the audit, which an owner then carries on
from by beginning a new epoch (ADR-0105).

## Read-only Recovery mode

Every open of the store runs SQLite's `PRAGMA quick_check` before any
migration. Damage met on the way to the check counts as the check
failing. A store that fails it, or whose migration fails, still opens,
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
`plane.sqlite3` and its journal files to `plane.sqlite3.damaged-<unix_ms>`
beside it, never deleting them, copies the snapshot into place, and
reopens it through the same checks as any open, migrating it when the
snapshot predates this release. A restored store that fails its own
checks leaves the Plane in Recovery mode with `recovery.restore_failed`.
On success the daemon start is recorded, the Security audit is validated
against the restored state, the restoration is recorded in the audit as
`recovery.snapshot_restored` with destructive risk, the search index
catches up, and the workers that were waiting run the start-time
reconciliation, including `jetfueld` reconnection, before serving as
usual. The reply names the snapshot and the damaged file:

```json
{"type": "recovery_snapshot_restored",
 "snapshot": "plane-1757000000000-daily.sqlite3",
 "damaged": "plane.sqlite3.damaged-1757000100000"}
```

The Security audit head is not restored with the snapshot. State moved
backwards past records the head names, so the next validation reports
`head_not_in_store` and the Plane is Security-degraded until an owner
begins a new audit epoch; that is the record of the restoration the
audit itself keeps (ADR-0105). The Deletion ledger of ADR-0102 arrives
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
