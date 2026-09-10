# Energy and idle resource budgets

Issue #45 implements ADR-0040 and ADR-0055. Energy policy controls admission;
it never terminates an admitted Run or removes pending input.

| Plane Setting | Default | Meaning |
| --- | --- | --- |
| `energy.concurrency` | 8 | Concurrent managed Run ceiling |
| `energy.low_power_concurrency` | 1 | Reduced ceiling, capped by the normal ceiling |
| `energy.constrained` | false | Force the reduced ceiling |
| `energy.foreground_override` | false | Explicitly allow user input above the ceiling |

Zero pauses new work. The override applies only to user input; it never promotes
a Scheduled-task or Auto-continue turn ahead of the queue. Setting changes use
the existing authenticated Commands and remain visible in the Event journal.
Starting and Stopping Runs consume slots until their terminal state commits.
Admission checks and reservations share a store transaction, including after
restart. New background turns on existing Runs also wait when the reduced
ceiling is exhausted.

Power is observed before admission. macOS uses `pmset`; Linux uses power-supply
status and the ACPI platform profile. Battery power and low-power mode select
the reduced ceiling. A failed observation also selects that ceiling and appears
as `unavailable` in the Capability snapshot. The observation takes at most one
second and leaves no resident probe. Active work rechecks power on its recovery
deadline; a Plane with only deferred input retries within sixty seconds.

Craft 1.9 adds the optional `subagents_limit` feature. Supporting Crafts accept
`constrain_subagents` before Start and before each later Turn, with policy and
Turn serialized together, and during an execution. A zero `max_children`
pauses new native children; null restores the Harness policy. Existing children
always continue. Under power constraints Jet reserves its reduced capacity for
Runs, so supporting Crafts receive zero rather than competing for spare slots.
The bundled Crafts currently report `monitor_only`; their native child activity
is visible through native events, but Jet cannot enforce its admission. A Craft
must declare the feature and support 1.9 before Jet reports `native_limits`.

Jet 1.33 adds Energy Settings, resource targets in Capability snapshots, and
native child-control reporting per Craft. Older peers receive their existing
snapshot shape. These are budgets, not measurements of current usage.

Idle maintenance waits for committed changes, indexed schedule deadlines, and
Craft retirement. Pending recovery work has a bounded retry deadline. An idle
Plane has no repeating repository scan or Utility timer. Crafts retire after
five idle minutes, as specified by ADR-0058. An unlaunched helper exits after ten
seconds. A completed helper retains only source still needed for acknowledgement;
terminal helpers preserve bounded final replay on disk after the shell exits.

Run `just resource-test` from `packages/` on each fixed macOS and Linux reference
machine, without other builds or tests running. The runner seeds ten thousand
Conversations and prints JSON measurements with OS and architecture. It enforces
35 MiB daemon RSS, 15 MiB per idle Craft, and 8 MiB per helper. Both bundled
Crafts and a helper owning an idle native process share the daemon's five-minute
window; their combined CPU must stay below 0.2%. Before measuring, the helper
processes one MiB of input/output across 32 acknowledgement cycles. The runner
also requires zero idle storage growth and no unneeded daemon children, then
closes the native input and verifies that its helper retires. The recipe first
runs the full 64-MiB spool backpressure/replay stress and bounded Craft recovery
checks. Use the same host,
OS build, and development Cargo profile when comparing measurements.

The ordinary suite also checks simultaneous admission, deferred turn identity,
native child controls, schedule deadlines, five-minute Craft retirement, bounded
Craft restart attempts, and the terminal's eight-MiB rolling replay limit. The
five-minute resource measurement is excluded from the ordinary suite; the
unlaunched-helper memory and retirement check runs normally. No release build is
needed for either recipe.

## Recorded baseline

On 2026-09-10, macOS 26.6.2 (25G83), aarch64, the development profile passed
with ten thousand Conversations and a 300.195-second idle window:

| Process | Peak RSS (KiB) | Limit (MiB) |
| --- | ---: | ---: |
| `jetd` | 12,528 | 35 |
| `jet-craft-codex` | 2,688 | 15 |
| `jet-craft-claude` | 2,784 | 15 |
| `jetfueld`, after output churn | 3,072 | 8 |

Combined measured CPU was 0.0% at `ps` time-counter resolution; no CPU-time
increase was recorded. Disk growth was zero, no unneeded daemon children
remained, and the helper retired after native input closed. The unlaunched
helper used 2,464 KiB and retired within its ten-second startup deadline.
All eight preceding replay and lifecycle stress checks passed. Linux has not
yet been measured; run the same recipe on the Linux reference host.
