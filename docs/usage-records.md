# Usage records

Issue #41 implements ADR-0023 through durable Usage records and one
`usage` Query (Jet protocol 1.29). A Craft reports what its Harness said
about consumption and about the Provider's quota windows (Craft 1.7); the
Plane stores the report beside the native event it came from, in the same
transaction as the rest of that turn's source.

Nothing here is a fleet record. A Plane answers for its own Account
bindings and says which Plane it is; grouping connected Planes into one
Provider account belongs to the GUI holding them, and only over the Planes
it is actually connected to (ADR-0016).

## What a record carries

| Field | Jet-observed consumption | Provider-reported quota window |
| --- | --- | --- |
| Source | The Harness's own accounting | The Provider's own accounting |
| Scope | One turn, or the Run so far | The Provider account, or one Model |
| Estimation | `measured` or `estimated` | `measured` or `estimated` |
| Finality | `interim` or `final` | Whether the window has closed |
| Account binding | The binding the Run selected, when it named one | Required |
| Model | Where the Harness names one | Where the window covers one |
| Conversation, Run | Always | Where the response was observed |
| Plane | The store itself, reported with every snapshot | The same |
| Time | When the Plane observed the report | The same |
| Native identity | The Harness's own usage identity, where it has one | The Provider's own name for the window |

Counts are `input`, `cached_input`, `output`, and `reasoning`. Tokens a
Provider wrote to its cache count as input, which is what they were
charged as. The Harness's own vocabulary is not flattened away: the
complete native event stays in the Event journal beside the record.

## Counting

One measurement is counted once. Within its Run a measurement is
identified by the Harness's native usage identity where there is one and
by the turn it covers otherwise, and repeating it **replaces** its record
rather than adding to it. An older repeat of a measurement already stored
changes nothing. A Craft names one measurement the same way every time it
reports it; a Craft that reported one turn first without its native
identity and then with it has described two measurements, and Jet has no
way to know otherwise.

A Craft may instead restate the Run's cumulative total, one per Model
where it breaks them down. A Run that reports these contributes them and
**not** its turns, which they already cover; a Run that reports turns
contributes their sum. The two are never added together.

Quota windows are kept as snapshot history rather than as one mutable
value. Reads select the freshest snapshot **per window**: a five-hour and
a weekly limit are two windows, never one sum, and so are two windows one
Provider spells the same way for two different Models. An unchanged Provider
response inside 15 minutes is a heartbeat and stores no new row; the
countdown to a window's reset is deliberately outside the change digest,
because it moves on its own.

## What the Query will not say

`usage` reports what the Plane can vouch for:

- A window whose Provider has not answered **about that window** within 15
  minutes — the interval ADR-0045 bounds an idle refresh by — is **stale**:
  history, not a current reading. Freshness follows the last answer about
  the window itself. Not the last time that answer changed, because an
  unchanged answer is a heartbeat against the window it repeats and is
  still an answer; and not the last time the Provider said anything at
  all, because a five-hour limit reported again says nothing about the
  weekly one beside it.
- A window whose own reset has passed is **stale** too. It describes a
  window that has already rolled over.
- A Provider that refused after a window was read is **unreachable**, with
  the reason the Craft gave. The last known fill is still shown, marked.
- How many contributing measurements Jet estimated, and how many can still
  change, are reported beside the total and per Model. The total itself is
  every deduplicated measurement; a client that wants only settled numbers
  reads the counts to know how much of it is not.
- A Conversation or Run answers with consumption alone. A quota window
  belongs to an Account binding, not to one Conversation.

A quota window a Run cannot attribute to an Account binding is not
recorded. There is no account on the Plane to attach it to, and attaching
it to a guess is exactly the total ADR-0023 forbids. Select a binding when
starting the Run (`start_visa_run`) to record windows for it.

## Bundled Crafts

| Harness | Consumption | Quota windows |
| --- | --- | --- |
| Claude Code | `result` totals, per Model where `modelUsage` breaks them down | `rate_limit_event` utilization, as the `unified` window |
| Codex | `thread/tokenUsage/updated` last-turn counts | `rateLimits.primary` and `.secondary`, as filled shares |

Codex's cumulative `tokenUsage.total` covers its whole thread, which a
resumed Conversation continues, so reporting it as this Run's total would
count earlier Runs again. It is left out. Claude Code's per-message
assistant counts are left out for the same reason: Jet could neither add
them up nor replace them without guessing.

A Provider that states a filled fraction rather than a countable limit
crosses the wire as a `share`: hundredths of a percent out of 10,000, so
no wire message carries a floating-point quota. A Craft reports the time
**remaining** on a window rather than an instant, and the Plane converts
it with its own clock.

`CraftUsage::Unreachable` reports a Provider that would not answer at all.
Neither bundled Craft sends it today; it exists for a Craft that asks a
Provider directly, and the Query already reports what it records.

**Jet does not poll a Provider.** ADR-0045's per-minute and fifteen-minute
cadence describes polling that has no source in v1: what is recorded is
what a Harness reported through its Craft while it was working. The
fifteen-minute bound is used here for what it can honestly govern — how
long an answer stands, and how often an unchanged one is stored again.
A Craft-driven Provider poll, and the manual refresh it would need, land
with the account panel.

## Boundaries

Every reported name is bounded metadata, not a payload: window names,
Model names, turn identities, and native usage identities are at most 128
characters and hold no control characters, and an unreachable reason is at
most 256. A report that is not that is refused rather than truncated
(ASVS 1.5.2, 2.2.1, 5.3.1). So is an amount above what the store holds,
which the wire's `uint64` permits and SQLite's `INTEGER` does not, and a
share above its fixed limit of 10,000. A Craft asserts no time, no Account
binding, and no Plane: the host stamps all three.

A window's stated length is a duration or nothing, so a Provider reporting
zero seconds — or a length no clock could mean — has stated none, and the
record says so rather than rejecting the report.

## What a refusal costs

A Usage report arrives inside its Run's own source batch, and a batch that
fails drops the Craft connection, records the Run as disconnected, and
never acknowledges the source, so a replay delivers the same report again.
A refused report is therefore refused on its own (issue #120). The report
is checked before anything is written; one the Plane cannot record is left
out of the batch, the native event beside it is journalled, the batch
commits, the source offset is acknowledged, and the Run goes on. The
refusal is written to the Diagnostic log as a `warn` record naming the Run
and the stable code (`usage.turn_unsupported`, `usage.amount_unsupported`,
`usage.share_unsupported`, and the rest) with nothing of the report
(ADR-0061).

Only a refusal of the reported content is survivable. A report already
admitted is the only kind that reaches the store, so a store integrity
error or a failed write there is the Plane's own failure and still fails
the batch, as any other write failure does.

## Review stages

The change is two stages. The first is durable state and its Query: the
`usage_observations`, `usage_quota_snapshots`, and `usage_provider_reach`
tables, the core that writes them under the counting rules above, and the
Jet 1.29 `usage` Query with its translation and client helper. The second
is reporting: the Craft 1.7 `usage` event, the SDK gate, and the two
bundled Crafts reading their Harness's own accounting. Generated schema
and GUI model updates are mechanical.

Time-series downsampling — the hourly and daily aggregates ADR-0045
retains after 90 days — is issue #116, below.

## History and retention

Issue #116 keeps the records inside the retention tiers ADR-0045 sets and
answers a time series from them (Jet protocol 1.43):

| Tier | Holds | Kept for |
| --- | --- | --- |
| Raw | `usage_observations`; `usage_quota_snapshots` other than the freshest of each window | 90 days |
| Hourly | `usage_aggregates` at `hour`, per Account binding and Model | 1 year, in whole UTC days |
| Daily | `usage_aggregates` at `day`, per Account binding and Model | Indefinitely |

History covers Jet-observed activity alone (ADR-0023). A Provider's quota
windows are readings rather than a series: the freshest snapshot of every
window is kept whatever its age, because it is what the `usage` Query
reports as the current reading, stale or not.

### Counting into an aggregate

An aggregate is never added to in place. Every write to a raw row marks
the hour of every row of its Run in `usage_dirty_hours`, and so does
removing a Conversation. The maintenance sweep recounts those hours from
the raw rows under the same rule the current totals use — one measurement
once, a cumulative Run total instead of the turns it covers — and then
re-sums the days those hours fall in from the hours. A measurement
**replaced** after it was first counted is counted as replaced: a Run's
cumulative total reported an hour after its turns leaves the turns' hour
empty and counts the total in its own.

Raw rows leave the store whole hours at a time, so a marked hour always
still has every row it had, and the recount is right however late it runs
— after Recovery mode, a degraded Security audit, or a daemon that was
offline. This is the one thing downsampling gives up: a measurement whose
raw row is already swept, which only a Run alive more than 90 days can
report again, comes back as a new row and is counted in both hours.

Removing a Conversation marks its hours too, so a Conversation forgotten
within the raw tier leaves the history it was counted into, while one
forgotten later stays in the aggregates it can no longer be recounted out
of. History is account-level accounting, not Conversation history; what a
forgotten Conversation consumed did happen, and only the raw rows carry
its identity.

### Sweeping

The sweep runs on every maintenance wake, recounts first, and then removes
what has left its tier, so nothing leaves a tier before the next one
carries it. Removing raw rows past 90 days changes no hour, and removing
hourly rows past a year changes no day. The `usage` Query answers current
totals from raw rows, so a Conversation whose observations were swept
answers zero there while its hours and days remain in the history. A wake
is scheduled for when the oldest row of any tier leaves it, and the sweep
is idle otherwise; it does not run in Recovery mode or while the Security
audit cannot be vouched for (ADR-0105).

### The `usage_history` Query

`usage_history` takes a `plane` or `binding` selection — aggregates carry
no Conversation or Run — a half-open `range` in Unix milliseconds, and a
`resolution` of `hour` or `day`. It answers one series per Model, in time
order, with empty buckets left out, from the tier that still holds the
whole range: a range that starts before the hourly tier's floor is
answered in days, and the answer's `resolution` says so. The bucket the
range starts inside is part of the answer. A range that
ends before it starts is refused as `usage.range_inverted`. The answer
names the Plane it covers and is fenced by the journal cursor it was read
at, like every other snapshot (ADR-0016, ADR-0092).

### Review stages

The change is three stages. The first is the store: the `usage_aggregates`
and `usage_dirty_hours` tables, the marking every raw write and removal
does, the rebuild, and the sweeps. The second is the core: the tiers, the
maintenance sweep and its deadline, and the history Query. The third is
the wire: the Jet 1.43 `usage_history` Query, its translation, and the
client helper. Generated schema and GUI model updates are mechanical.
