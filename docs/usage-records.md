# Usage records

Issue #41 implements ADR-0023 through durable Usage records and one
`usage` Query (Jet protocol 1.28). A Craft reports what its Harness said
about consumption and about the Provider's quota windows (Craft 1.8); the
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

- A window whose Provider has not answered within 15 minutes — the
  interval ADR-0045 bounds an idle refresh by — is **stale**: history, not
  a current reading. Freshness follows the last time the Provider
  answered, not the last time its answer changed, because an unchanged
  answer is stored as a heartbeat and is still an answer.
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
(ASVS 1.5.2, 2.2.1, 5.3.1). A Craft asserts no time, no Account binding,
and no Plane: the host stamps all three.

## Review stages

The change is two stages. The first is durable state and its Query: the
`usage_observations`, `usage_quota_snapshots`, and `usage_provider_reach`
tables, the core that writes them under the counting rules above, and the
Jet 1.28 `usage` Query with its translation and client helper. The second
is reporting: the Craft 1.8 `usage` event, the SDK gate, and the two
bundled Crafts reading their Harness's own accounting. Generated schema
and GUI model updates are mechanical.

Time-series downsampling — the hourly and daily aggregates ADR-0045
retains after 90 days — is not in this change. The records it aggregates
are here, and the Query answers current totals; the tiers and their sweep
land next.
