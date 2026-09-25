# Core distribution

Issue #57 implements ADR-0026, ADR-0053, ADR-0054, ADR-0057, ADR-0059,
and ADR-0060: one release payload per architecture holding the four
role executables, a per-user version layout with drain, activation, and
rollback across the supported release pair, and release checks that fail
when an executable leaves its accepted dependency or size envelope.

## The payload

A payload is one directory named `jet-core-<version>-<label>` holding
`jetd`, `jetfueld`, `jet-craft-claude`, `jet-craft-codex`, a
`SHA256SUMS`, and `manifest.json`:

```json
{"version": "0.2.0", "target": "universal-apple-darwin",
 "protocol_major": 1, "protocol_minor": 42,
 "schema_version": 20260914164556,
 "executables": {"jetd": "<sha256>", "jetfueld": "<sha256>",
                 "jet-craft-claude": "<sha256>", "jet-craft-codex": "<sha256>"}}
```

The version is the one Jet release version every bundled product shares
(ADR-0053); the protocol and schema fields are what `jetd core describe`
reports for the build and are what activation reasons about. Labels are
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and
`universal-apple-darwin`, whose executables carry an `arm64` and an
`x86_64` slice. Crash symbols ship separately as
`jet-core-symbols-<version>-<label>` with a `.debug` file per executable
on Linux and a `.dSYM` bundle on macOS; the payload executables are
stripped. The Linux release `jetd` is also linked without
`.eh_frame_hdr` (`jet-daemon/build.rs`). It aborts on panic, so the
table only served in-process backtraces: `RUST_BACKTRACE` now prints
the panic message and location without frames. `.eh_frame` stays, so
gdb, eu-stack, and systemd-coredump still walk a core dump with or
without the symbols. Every Linux executable packs its relative
relocations into `DT_RELR` (`packages/.cargo/config.toml`), so it
refuses to load on glibc older than 2.36; built on Ubuntu 24.04 the
executables already need glibc 2.39.

`just release-package --target <triple>` builds all four executables
with the `release-small` profile (ADR-0059), splits and strips symbols, writes the manifest by running the
freshly built `jetd core describe`, and archives payload and symbols.
Two Apple targets given together are merged with `lipo` into one
universal payload. `.github/workflows/release.yml` runs it for the three
labels on every version tag matching the workspace, gates all three payloads,
and publishes their archives and checksums together as a GitHub release. Code signing and
notarization belong to the GUI distributions and run after these
payloads exist. Stable releases update `apexgang/homebrew-tap` using an
Ape Bonker installation token scoped to that repository. Prereleases leave
the stable formula unchanged. See [CI and releases](../.github/workflows/README.md)
for credentials and retry instructions.

`brew install apexgang/tap/jet` installs the compiled payload without Rust.
`brew services start apexgang/tap/jet` runs `jetd serve --channel homebrew`.
The service definitions preserve helpers across daemon restarts.

## The release envelope

`.github/packaging/release.toml` is the accepted envelope and
`.github/packaging/release-baseline.json` the accepted measurements per label.
`just release-check --target <label>` fails when any of these hold:

- A stripped executable exceeds its per-architecture budget of 12 MiB
  for `jetd`, 3 MiB for `jetfueld`, or 6 MiB per Craft, or the four
  together exceed 30 MiB for one architecture (ADR-0054). Universal
  executables are measured per slice.
- An executable grew more than five percent over its accepted size. `just
  release-accept --target <label> --justification "<why>"` records the
  new sizes, and the change to the baseline file is what a reviewer sees.
  A label with no accepted sizes yet is checked against the budgets only.
- A role links outside its seam: `jetfueld` and the Crafts must not
  reach SQLite, `reqwest`, `age`, the store, or the core, and
  `libsqlite3-sys` must be pinned exactly once and reached only through
  `jet-store`, `jet-core`, and `jet-daemon` (ADR-0057, ADR-0060). The
  check reads `cargo tree`, so `just release-envelope` runs it without a
  release build.
- An executable still carries symbols, has no crash-symbol artifact, or
  a release profile in `Cargo.toml` no longer says what ADR-0059
  requires.

Measured on Linux x86_64 at this change and accepted in
`release-baseline.json`:

| Executable | Profile | Stripped size | Budget |
| --- | --- | --- | --- |
| `jetd` | `release-small` (opt-level `z`, fat LTO) | 12,342,792 bytes (11.77 MiB) | 12 MiB |
| `jetfueld` | `release-small` | 1,019,248 bytes (0.97 MiB) | 3 MiB |
| `jet-craft-claude` | `release-small` | 1,745,624 bytes (1.66 MiB) | 6 MiB |
| `jet-craft-codex` | `release-small` | 1,781,616 bytes (1.70 MiB) | 6 MiB |

The payload totals 16.11 MiB against 30 MiB. `jetd` has 240,120 bytes
(1.9%) of room on this slice. Issue #215 brought it
down from 18.31 MiB (19,203,248 bytes at 0fde750, as #215 records).
The changes:

- `jetd` builds with `release-small` instead of `release` (opt-level
  `s`, thin LTO); ADR-0059 records the change in its 2026-09-25
  amendment. The workspace's development profile, under which the
  ADR-0022 and ADR-0055 budgets are measured, is already opt-level `z`
  with fat LTO.
- Schedules resolve zones with jiff and its bundled tz database instead
  of chrono-tz. The zone data moves from tzdb 2025b (chrono-tz 0.10.4)
  to 2026c (jiff-tzdb 0.1.8). Under 2026c Morocco stays at +00 from
  2026-09-20, and British Columbia and Alberta stay at -07 and -06 from
  2026-11-01. Schedules in `Africa/Casablanca`, `Africa/El_Aaiun`,
  `America/Vancouver`, `America/Edmonton` and their links fire at the new
  instants. A firing persisted before the upgrade keeps its instant
  (ADR-0080); every later one follows 2026c.
- clap drops colored output and "did you mean" suggestions, and
  release builds compile out the `log` and `tracing` call sites in
  dependencies.
- The bundled SQLite leaves out FTS3, R*Tree, dbstat, soundex, and
  extension loading.
- Relative relocations are packed into `DT_RELR`, and the Linux `jetd`
  has no `.eh_frame_hdr`. Neither applies to macOS.
- Store transactions, the blocking filesystem helper, and reply
  classification in the daemon and the No-Visa client are compiled once
  instead of once per caller or per reply kind.

TLS stays on rustls with reqwest's aws-lc-rs provider, so every HTTPS
connection `jetd` makes, including the GitHub API calls that carry the
user's token, keeps the hybrid post-quantum X25519MLKEM768 key
exchange. rustls is at 0.23.45, which fixes RUSTSEC-2026-0285. The ring
provider would save 668,968 bytes on Linux x86_64 but offers only
X25519, P-256, and P-384.

CI measured the stripped `jetd` on every slice with these changes under
both profiles, with rustls on the ring provider #215 tried at the time:

| Slice | `release` (opt-level `s`, thin LTO) | `release-small` (opt-level `z`, fat LTO) |
| --- | ---: | ---: |
| Linux x86_64 | 13.32 MiB | 11,673,824 bytes (11.13 MiB) |
| Linux aarch64 | 12.39 MiB | 9,653,504 bytes (9.21 MiB) |
| macOS x86_64 | 13.36 MiB | 9,987,376 bytes (9.52 MiB) |
| macOS arm64 | 12.24 MiB | 7,640,832 bytes (7.29 MiB) |

Every slice was over 12 MiB under `release` and under it with
`release-small`. With aws-lc kept, only Linux x86_64 has been measured
here; the other slices had at least 2.4 MiB of room, and
`.github/workflows/release-size.yml` measures all three labels with the
configured profiles on every pull request that touches the core and
fails when a slice is over budget. Identical code folding
(`-Wl,--icf=all` with rust-lld) would remove about 110 KiB more under
fat LTO and is not applied.

## Versions under the Jet home

A GUI-managed installation keeps immutable versions under the Jet home
(ADR-0026):

```text
~/.jet/core/
  versions/<version>/   read-only executables and manifest.json
  current -> versions/<version>
  previous -> versions/<version>
```

The service definition starts `~/.jet/core/current/jetd serve --channel
gui`; `.github/packaging/linux/jetd.service`, `.github/packaging/linux/jetd-autostart.desktop`,
and `.github/packaging/macos/com.apexgang.jet.jetd.plist` are the systemd user
unit, the XDG autostart fallback, and the LaunchAgent the macOS app
registers through `SMAppService`. Both service definitions signal `jetd`
alone and never its process group, because `jetfueld` helpers must
outlive it, and both restart it after the clean exit a drain ends in, so
an activation is followed by the daemon `current` now names.

`jetd core` is the lifecycle the GUI updater drives. Every subcommand
prints one JSON object; `3` means refused, `4` means the owner did not
relinquish the Plane in time, `1` any other failure.

- `stage --payload <dir>` verifies every executable against the
  manifest, copies the payload under a dot-prefixed name, makes it
  read-only, and renames it into `versions/`. Staging the same release
  again is accepted; a different payload under a staged version's name
  is refused.
- `activate --version <v>` runs the compatibility rule, then the channel
  policy, then drains the owning `jetd` with `SIGTERM` and waits for the
  operating-system lifetime lock to be free, and only then switches
  `current` and points `previous` at what `current` named. It reports
  the drained daemon and how many helpers were alive. It does not start
  the new daemon; the service manager does.
- `rollback` is `activate` for whatever `previous` names.
- `drain` only asks the owner to leave and waits for the lock.
- `status` lists staged, current, and previous versions, live helpers
  with their deployed versions, and the Plane's owner.
- `prune` removes every staged version other than the pair.

The compatibility rule keeps the supported current/previous pair honest:

- The candidate must be built for the current release's target.
- While any helper is alive, the candidate must speak the same protocol
  major as `current`, or as the `jetd` applying the rule on a Plane with
  no version layout yet, because a live execution keeps its negotiated
  major for its whole lifetime (ADR-0088).
- The candidate must know every migration the store has applied, unless
  it is exactly the release `previous` names. Schema changes are
  expand-only for one release, so the previous release can open a store
  the current one migrated, and nothing older may (ADR-0073).

The channel policy makes a channel change explicit: a daemon whose lock
metadata says it is Homebrew-managed or a development build is drained
only when `--replace-channel` names that channel. Homebrew's own
lifecycle otherwise stays with `brew services`, and a Homebrew daemon
never self-updates.

Active helpers are preserved by construction. `jetd` copies `jetfueld`
into each execution's runtime directory when it launches, so a helper
keeps the executable of the release it started under until it exits,
whichever version `current` names, and pruning an old version cannot
remove a running helper's executable. After activation the new daemon
reconnects to those helpers through the recovery path that already
survives a daemon restart.

## Tests

`packages/jet-daemon/tests/installation.rs` drives the public surface:
staging, activation, rollback, and pruning with real executables;
activation while a Run is waiting for approval, which drains the owning
daemon, leaves the helper alive, and lets the daemon started from
`current/jetd` finish the Run; and the refusals for another channel, a
pinned protocol major, a schema outside the pair, and a foreign target.
Unit tests cover the manifest rules, the version layout, and the
compatibility rule table.
