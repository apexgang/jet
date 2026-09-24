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
All eight preceding replay and lifecycle stress checks passed.

On 2026-09-15, Linux x86_64 (CachyOS, kernel 7.2.4), the development profile
passed with ten thousand Conversations and a 300.093-second idle window:

| Process | Peak RSS (KiB) | Limit (MiB) |
| --- | ---: | ---: |
| `jetd` | 18,948 | 35 |
| `jet-craft-codex` | 4,328 | 15 |
| `jet-craft-claude` | 4,348 | 15 |
| `jetfueld`, after output churn | 4,164 | 8 |

Combined measured CPU was 0.0%, disk growth was zero, no unneeded daemon
children remained, and the helper retired after native input closed. The
unlaunched helper used 3,708 KiB and retired within its deadline. All eight
preceding replay and lifecycle stress checks passed.

## Desktop (Linux)

The Linux desktop journey (Wave 4 spec F, `apps/jet-tauri/tests/e2e/`)
measures the installed release bundle, not a development build.
`.github/workflows/desktop-e2e.yml` runs it on a fresh `ubuntu-24.04`
runner, against the unsigned x86_64 `.deb` from `packaging.yml` or the
signed one before the release workflow publishes. The app runs under Xvfb
with software rendering, next to a real systemd user session, and
tauri-driver and WebKitWebDriver drive it. Every run uploads
`desktop-e2e.json` in the `jet-desktop-e2e-<label>` artifact, with
screenshots and a summary table. Only the journey's functional checks fail
the job. These measurements never do.

| Measurement | How it is taken |
| --- | --- |
| Launch to `shell-interactive` | From the `jet-tauri` process's start (its `/proc` start time, one clock tick of resolution) to the shell's `shell-interactive` User Timing mark, placed in the first frame after the sidebar and the composer mount. The first launch after installing is the cold sample; each relaunch is a warm one (three per run). The installed files are already in the page cache, so the cold sample is a first launch, not a cold-cache start. |
| Provisioning | From the process start to the main window's `local-service-running` mark, and from its `local-service-checking` mark to `running`. On a fresh install this covers extracting the bundled payload, `jetd core stage` and `activate`, writing and enabling `jetd.service`, and the daemon answering. |
| Idle CPU | After provisioning and 15 quiet seconds, `utime + stime` over 60 seconds, as a percentage of one CPU. The app tree is `jet-tauri` and everything below it: WebKitWebProcess, WebKitNetworkProcess, and any sandbox helper. The daemon tree is the unit's main process and its children. |
| Idle memory | PSS from `/proc/<pid>/smaps_rollup`, summed over each tree every two seconds of the same window: mean, maximum, and last. PSS divides shared pages among the processes that map them, so the sum does not count the WebKit libraries once per process. When the kernel hides a process's memory from the user, the report lists that process as unreadable and leaves it out of the sum. |
| Reconnect | `systemctl --user kill jetd.service` sends SIGTERM, and `Restart=always` with `RestartSec=2` starts a new daemon. The journey records when the daemon socket accepts connections again and when the UI's next `local-plane-connected` mark lands. The ceiling applies only to the time after the daemon is ready. The restart delay dominates the total, and the report keeps it separate. |

The journey polls every 100 ms, so the daemon-ready time has that
resolution. The marks and the process start time do not depend on polling.
A signed build checks github.com for an update 10 seconds after launch,
which can land in its idle window. Unsigned bundles have no updater.

### Proposed ceilings

These are proposals, and nothing enforces them yet. CI runners are
shared machines with software rendering, so a CI value that crosses a
ceiling is a signal to measure on a reference host, not a failure.

| Measurement | Proposed ceiling |
| --- | --- |
| Launch to `shell-interactive`, cold | p95 ≤ 2.5 s |
| Launch to `shell-interactive`, warm | p95 ≤ 1.0 s |
| Idle CPU, app tree | ≤ 0.5% of one CPU over 60 s |
| Reconnect after the daemon is ready | ≤ 1.5 s |
| Idle PSS, app tree and `jetd` | None yet. Record a baseline first. |

A p95 needs repeated runs. One CI run gives one cold sample and three warm
samples. Take the p95 over at least 20 runs on one reference host before a
ceiling becomes a gate. The journey provisions the real `~/.jet` and systemd
user unit of whoever runs it, so a reference host must be a throwaway
machine or VM. `bun tests/e2e/main.ts --disposable-machine …` allows that
outside GitHub Actions.

### Recorded measurements

None yet. As of 2026-09-24 the journey has not run on GitHub Actions. Only
its dry run against fakes (`just e2e-dry-run`) and its unit tests have run.
Record the first CI values here from the `jet-desktop-e2e-x86_64-unknown-linux-gnu`
artifact, with the run link, runner image, and WebKitGTK version from the
report's `environment`. Record reference-host values separately.
