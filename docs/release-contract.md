# The core v1 release contract

Issue #58 asks the completed Jet core to demonstrate, at its public protocol
seam, that it satisfies issue #3: its supported Codex and Claude Code
behavior, its failure semantics, its security boundaries, and its resource
budgets (ADR-0012, ADR-0022, ADR-0032, ADR-0054, ADR-0104). This document
names the gate behind each acceptance criterion, how to run it, and what it
last reported. A criterion is met only when its gate passes on both
supported core platforms, Linux and macOS (ADR-0032).

## Test gates

`just release-contract` runs everything below that needs no reference
host and no GUI toolchain: the ordinary suite, clippy, the migration
conventions, the dependency envelope, and the contract drift check. The
`Core tests` workflow runs the same on `ubuntu-latest` and `macos-latest`
when core inputs change in a pull request or push to `main`. The `Wire
contracts` workflow checks schema drift and the Rust, TypeScript and Swift
corpora when their inputs change. CI uses the fast development-profile
overrides in `.github/ci/cargo.toml`; release profiles remain optimized by role.
The reference measurements
below run on the fixed reference hosts by hand, never on shared runners.

| Category | Gate |
| --- | --- |
| Current-and-previous protocol fixtures | `just contracts-test`: the shared corpora in `jet-protocol/contracts/*-fixtures.json` through the Rust, TypeScript, and Swift readers, and `jet-protocol/tests/compatibility.rs` for current/previous-major negotiation. There is one released protocol major; every `jet-daemon/tests/*.rs` gated on a `*_MINOR` constant drives a peer one minor below the feature it exercises |
| Parser fuzzing and hostile input | `jet-protocol/tests/hostile_input.rs`: a fixed corpus of malformed control envelopes and frame headers that must be refused, and deterministic mutation of every contract fixture through the decoders the `jet-protocol-fuzz` libFuzzer targets drive. `cargo fuzz run control` and `cargo fuzz run frames` remain the open-ended form |
| Multi-client races | `jet-daemon/tests/commands.rs`, `conversations.rs`, `protocol_streams.rs`, and `turns.rs`: concurrent Commands settle one revision order, a concurrent write stales pagination, one connection serves concurrent Queries, and concurrent admissions keep order across restart |
| Subprocess failure injection | `jet-daemon/tests/recovery.rs`, `turn_recovery.rs`, `jet-craft-sdk/tests/process.rs`, `jet-fueld/tests/replay.rs` and `native_input.rs`: Craft crashes retried at the pinned digest with a bound, dead executions marked lost, unmatched helpers orphaned, incompatible restarts refused, spool backpressure and replay |
| Git fixtures | `jet-daemon/tests/projects.rs`, `git_delivery.rs`, `promotions.rs`, `workspaces.rs`, `forks.rs`, and `no_visa/tool_tests.rs`: registration verdicts including linked worktrees, submodules, and LFS, three-way promotion preflight, idempotent delivery Effects, and worktree redirection refusal |
| Recovery | `jet-daemon/tests/store_recovery.rs` and `recovery.rs`, `jet-store` `snapshot`, `recovery`, and `deletion` tests, `jet-core` `store_recovery`: damaged stores served read-only until a verified snapshot is restored, the Deletion ledger reapplied |
| Migrations | `just sqlx-migrations-check`, `just sqlx-check`, and `jet-store/src/migrations.rs`: a store at the previous release's schema opens through this build and migrates forward (ADR-0073); an older `jetd` opens a newer store by skipping versions it does not know |
| Harness conformance | `just test -p jet-craft-codex` and `just test -p jet-craft-claude` drive a real Craft, a real `jetfueld`, and a controlled native peer |

## Conformance matrices

[conformance-matrix.md](conformance-matrix.md) classifies every
Harness-facing v1 capability for Codex CLI 0.153.4 and Claude Code 2.1 as
native, Jet-equivalent, generic fallback, or unavailable, and names why
each unavailable row is not a release blocker. A unit test in each Craft
fails when the pin behind its unverified-compatibility warning is not the
one the matrix publishes.

## Budget gates

| Budget | Gate | Baseline |
| --- | --- | --- |
| Store, startup, reconnect, ingestion (ADR-0022) | `just budget-test` on a reference host, alone: `jet-daemon/tests/budgets.rs` seeds 10,000 Conversations and one million journal Events and writes `target/budgets/<os>-<arch>.json`; `just budget-check` fails a measurement over its limit in `budgets.toml` or more than 15% worse than the accepted one, beyond any `tolerance` the limit names for scheduling jitter. Reconnect paging is measured at the store seam, in the store's bounded pages, without protocol framing | `budget-baseline.json`, per operating system and architecture, through `just budget-accept --justification` |
| Binary size (ADR-0054) | `just release-check --target <label>` after `just release-package`; the `Release core executables` workflow on every `v*` tag | `release-baseline.json`, drift over 5% fails |
| Idle resources (ADR-0055) | `just resource-test` on a reference host, alone: 35 MiB daemon RSS, 15 MiB per idle Craft, 8 MiB per helper, 0.2% combined CPU over five minutes, zero idle storage growth | Recorded in [resource-budgets.md](resource-budgets.md) |
| Disk pressure (ADR-0079) | `jet-core` unit tests in `disk_pressure.rs`, `artifact/mod.rs`, and `checkpoint/mod.rs`: pressure rejects new Runs without poisoning retries or reads, the disposable budget reserves and releases uploads, and the current diff stays readable when pressure prevents Artifact ingestion | Behavioral, no measurement |
| Stress | `just resource-test` first runs the 64 MiB spool backpressure and replay stress and the bounded Craft recovery checks; the ordinary suite runs the smoke-scale budget runner so it cannot rot | Behavioral, no measurement |

The ordinary suite runs the budget runner at a smoke scale of 100
Conversations and 2,000 Events, asserting only that every gate produces a
measurement. Reference measurements use the development profile the
justfile selects, on the same host, OS build, and profile as the accepted
baseline.

## Recorded measurements

Linux x86_64, development profile, on 2026-09-21, with 10,000
Conversations and 1,000,000 Events:

| Measurement | Value | Limit |
| --- | ---: | ---: |
| `daemon_ready_ms`, first start of the day after a clean shutdown | 17 | 150 |
| `daemon_ready_unclean_ms`, second start after a kill | 422 | informational |
| `store_open_ms` | 1.5 | informational |
| `commit_64_events_p99_ms` | 1.7 | 10 |
| `commit_256_kib_p99_ms` | 2.7 | 10 |
| `sidebar_page_p95_ms` | 1.9 | 10 |
| `blocks_500_p95_ms` | 2.8 | 15 |
| `reconnect_10000_events_ms` | 62 | 100 |
| `ingestion_events_per_second` | 56,760 | 10,000 |

Every gate passes with room, and the Linux label is accepted. Before
#152 the ready time measured 2,099 ms on the first start of a day and
535 ms on the second, because `Store::open` ran `PRAGMA quick_check` over
the whole file on every open, the first start of a day copied and
verified the day's Recovery snapshot before the ready line, and the
search index walked every operational Event behind the last semantic one
on every start. Now the check follows an unclean shutdown alone, which
the write-ahead log SQLite leaves behind marks and which
`daemon_ready_unclean_ms` still shows; the day's snapshot and the
destructive maintenance it precedes run behind the ready line
(ADR-0097); and semantic Events have an index of their own. macOS has not
been measured yet; run the same recipe on the macOS reference host and
accept its label.

## What still fails the contract

- `jetd` measures 17.08 MiB stripped on Linux x86_64 against its 12 MiB
  budget, so `just release-check` fails ([core-distribution.md](core-distribution.md)).
- The Codex Craft leaves three capabilities the Harness exposes unused:
  native resume, Model selection, and a No-Visa tool bridge. ADR-0104 makes
  them release blockers, marked as Craft gaps in the matrix (#154).
- Four `jet-daemon` targets fail on `macos-latest` in the first `Core
  tests` run: the deletion ledger, reviews, store recovery, and utility
  suites (#156). They pass on Linux.
- Reference performance has a Linux baseline only; the macOS label still
  has to be measured and accepted.
