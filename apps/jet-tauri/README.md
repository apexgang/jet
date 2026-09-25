# Jet desktop for Linux

The Linux client uses Tauri 2, Svelte 5, and the shared Jet protocol. It is an
independent application: its native Rust shell talks to `jetd` through
`packages/jet-client`; it does not call or link the Swift application.

The cross-client architecture and frozen Wave 0 boundary are recorded in
[ADR-0106](../../docs/adr/0106-freeze-independent-desktop-client-foundations.md).

## Windows and capabilities

The app has two windows, each with its own capability. Commands a window is
not granted are refused by the Tauri ACL.

- `main` (`main-plane-setup`): Plane feeds, Planes and pairing, Project setup,
  tasks and Runs, the work panel, terminals and delivery. It renders agent
  output, so it cannot change accounts, Crafts, extensions or Plane settings.
  It can open Settings (`open_settings`) and read desktop preferences.
- `settings` (`settings-window`): device preferences, notification routing,
  read-only Plane summaries, and reviewed Plane setting, Account,
  Auto-continue, Craft and Harness extension changes. It never receives a
  Plane feed or agent content, and has no dialog, core or plugin permission.
  The shell opens the local Craft file dialog natively.

`src-tauri/src/jet/mod.rs` has a manifest test that keeps `lib.rs`,
`build.rs` and both capability files in agreement.

## Boundary

- The native Rust shell talks to every Plane through `jet-client`. Socket
  paths, SSH arguments, client identity, keys, credentials, connection proofs,
  Command bodies and Event payloads stay native.
- The webview holds only opaque handles: Plane IDs, snapshot and review IDs,
  and entry and source tokens. It never sends a path, a secret or a
  confirmation body back.
- `jetd` decides every policy. The shell validates shape and size, and every
  change is reviewed before it is sent.
- Wave records: [2.3](docs/wave-2.3.md), [3.1](docs/wave-3.1.md),
  [3.2](docs/wave-3.2.md), [3.3](docs/wave-3.3.md), [3.4](docs/wave-3.4.md).
- Accepted dependency advisories and why they are safe:
  [dependency advisories](docs/dependency-advisories.md).

## Verify

Run these commands from this directory:

```sh
just install
just check
just audit
just bundle
```

CI runs `just check` and `just audit` for every change to this app. `just audit`
needs `cargo-deny` and network access.

The release journey in `tests/e2e/` installs the built `.deb` on a fresh CI
runner and drives the real app through WebDriver. It checks provisioning,
Settings › Versions, a daemon restart and a relaunch, and records launch, idle
and reconnect measurements. The app it launches provisions the real `~/.jet`
and systemd user unit, so the journey runs only in CI
(`.github/workflows/desktop-e2e.yml`). `just check` includes
`just e2e-dry-run`, which runs the journey against fakes and never touches your
home.
