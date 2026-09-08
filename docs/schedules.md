# Scheduled tasks

Issue #43 implements ADR-0024 and ADR-0080 through Jet protocol minor 20.

`create_schedule` takes `conversation_id`, an original IANA `time_zone`, a
`local_time` in `HH:MM:SS` form, and a `prompt` of 1 to 8192 UTF-8 bytes.
The rule fires daily. A retained Conversation supports up to 32 enabled tasks.
The result includes the immutable `schedule_id` and the next firing's
`firing_id`, `intended_local`, and selected `due_at_unix_ms`.

Creation selects the first occurrence strictly after the current instant.
Nonexistent local times advance to the first valid instant after the gap,
including half-hour transitions and skipped dates. Repeated times use their
earlier occurrence once. Firing identities derive from the schedule identity
and original intended local date and time. Each next UTC deadline is persisted;
recovery uses that selection even when the installed time-zone rules differ.
Advancement proceeds from the stored intended local date, never from a
recomputed wall-clock cursor.

Each firing submits a turn to its owning Conversation. The first `start_run`
must select and pin a Craft before queued input can execute. Later firings use
that accepted pin, with the same capability and working-root checks as other
[queued turns](turn-queue.md). Background work never chooses a Craft.

Only the newest missed firing within seven days enters the schedule slot.
A newer firing replaces pending scheduled work, receives a new Conversation
sequence, and leaves user turns and active work intact. A full user queue keeps
the firing pending until admission is possible. Catch-up advances at most 128
occurrences per task per tick, retaining an outcome for each. A scheduled turn
cannot claim execution while a newer firing remains due or its seven-day
window has expired.

The `scheduled_tasks` Query returns the enabled tasks for one Conversation and
an Event cursor from the same read transaction. `cancel_schedule` takes a
schedule identity, removes future firings, and withdraws its pending turn.
Active work continues. Rules are immutable; cancel and create a task to change
its prompt, local time, or zone. Exact Command retries replay their receipt.

`schedule.created`, `schedule.canceled`, and `schedule.fired` Events retain
schedule decisions. Firing outcomes are `queued`, `superseded`, or `expired`.
Later `turn.changed` Events use the firing identity as their Turn identity and
record execution, replacement, or cancellation. Scheduled admissions carry a
`scheduled_task` Event origin and the authorizing Client identity. Older
protocol minors receive the generic Event payload without the new origin
variant. Expiration records an explicit `expired` firing outcome.

Enabled tasks require retained Conversations, and their store references
prevent Conversation deletion. Future autodelete eligibility must also check
these enabled tasks before cleaning their Workspaces; Workspace autodelete is
not implemented yet.

Persistence is a separate migration and store module. The remaining change
crosses the core, queue claims, protocol, and daemon recovery; those integration
changes must land together to provide executable schedules. Generated client
models follow the protocol change. Verification covers civil transitions,
restart, offline catch-up, queue pressure, and a real daemon with a busy Run.
