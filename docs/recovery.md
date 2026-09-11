# Recovery snapshots

Issue #49 implements ADR-0097, ADR-0073, and ADR-0077. A Recovery
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

## Validation

`just test -p jet-store snapshot` covers naming, verification,
publication, the once-per-day rule, and retention tiers.
`just test -p jet-core store_recovery` covers the daily and maintenance
moments through the core.
