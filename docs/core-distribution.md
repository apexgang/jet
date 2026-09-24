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
stripped.

`just release-package --target <triple>` builds `jetd` with the
`release` profile and the helper and Crafts with `release-small`
(ADR-0059), splits and strips symbols, writes the manifest by running the
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

`brew install apexgang/tap/jetd` installs the compiled payload without Rust.
The formula is `jetd` because homebrew/core already has an unrelated `jet`.
`brew services start apexgang/tap/jetd` runs `jetd serve --channel homebrew`.
The service definitions preserve helpers across daemon restarts. The systemd
unit starts only while the formula's `jetd` exists, so a unit left behind by
an uninstall cannot restart in a loop.

## The Linux desktop app

Each release also bundles the Tauri desktop app for both Linux labels as a
deb, an rpm, and an AppImage. Every bundle carries the label's unmodified
`jet-core-<version>-<label>.tar.gz` archive, never loose executables: the
AppImage build rewrites every ELF file under `usr/lib`, which would break
the digests `jetd core stage` checks. On first launch the app extracts the
archive, stages and activates it under `~/.jet/core`, and installs the
systemd user unit or the autostart entry described in
[Versions under the Jet home](#versions-under-the-jet-home), so these
installs run the `gui` channel. The bundles are signed for the Tauri updater, and `latest.json`
on the latest release lists them.

The `apexgang/tap/jet-app` cask installs the AppImage on Linux and depends on
`apexgang/tap/jetd`, so the Homebrew channel owns the daemon, `brew services`
runs it, and the app's updater stays off. It needs Homebrew 6.0 or later.
Homebrew trusts only the tap items named on the command line, so install
both together:

```sh
brew install apexgang/tap/jetd apexgang/tap/jet-app
```

or run `brew trust apexgang/tap` first. See
[CI and releases](../.github/workflows/README.md#releases) for how the
bundles, the cask, and the updater manifest are built and published.

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

Measured on Linux x86_64 at this change:

| Executable | Profile | Stripped size | Budget |
| --- | --- | --- | --- |
| `jetd` | `release` (opt-level `s`, thin LTO) | 17.08 MiB | 12 MiB |
| `jetfueld` | `release-small` (opt-level `z`, fat LTO) | 1.02 MiB | 3 MiB |
| `jet-craft-claude` | `release-small` | 1.71 MiB | 6 MiB |
| `jet-craft-codex` | `release-small` | 1.63 MiB | 6 MiB |

`jetd` is over its budget, so `just release-check` and the release
workflow fail on it until the daemon is smaller; the workflow still
uploads the failed payload for inspection. Under the size-first
profile (opt-level `z`, fat LTO) the same `jetd` measured 14.09 MiB, so
the profile choice is not what puts it over; the budget needs dependency
work, which this change does not attempt. `just release-accept` never records a size
over budget, so the baseline file holds no `jetd` size and the drift
rule cannot anchor above the limit.

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
