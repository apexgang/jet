# Wave 4: Release hardening (Tauri / Linux)

The Linux app now ships as a deb, an rpm and an AppImage, and each carries
the core payload. On first launch the app installs the Jet service from that
payload and starts it, unless Homebrew or a development build already runs
it. Signed releases update themselves through the Tauri updater. A `v*` tag
bundles, signs, checks and publishes the app next to the core, then writes
the `jet` formula and the Linux-only `jet-app` cask to
`apexgang/homebrew-tap`. The wave also fixes the hardening defects D1 to D6
and D8.

Nothing has been released yet. `jetd` is over its 12 MiB budget (#215), so
the release workflow cannot publish. The desktop journey and the Homebrew
check, which exercise the installed app and the tap on a real runner, have
never run in CI. Every native path below that touches systemd, XDG
autostart, Homebrew, pkexec or GitHub was verified against fakes only.

## Delivered

### Automatic core installation (§A)

`src-tauri/src/jet/local_service/` owns the Jet service on this computer
(ADR-0026). At launch, and on Repair, one provisioning pass observes the core
natively, runs the pure decision table in `decision.rs`, and carries out its
plan. The webview reads a `LocalServiceView` and may ask for a repair or a
reviewed rollback. It never names a path, a version or a command, and the
view carries no path, pid or native text.

**Channels.** `jetd core status --home ~/.jet` names the owner of the
Plane's lifetime lock and its channel. The app runs it with the first `jetd`
it finds: `~/.jet/core/current/jetd`, then the Homebrew keg's, then the one
in the unpacked bundled payload.

| Channel | Who runs `jetd` | What the app may do |
| --- | --- | --- |
| `gui` | This app's layout under `~/.jet/core`, started by `jetd.service` or the XDG autostart entry | Install, update, start, roll back |
| `homebrew` | The `apexgang/tap/jet` formula under `brew services` (unit `jet-homebrew`) | `brew services start` when it is stopped, nothing else. App updates are off |
| `development` | A `jetd` run from a checkout | Nothing |

**Decision table** (`decide` in `decision.rs`). "Unknown" means no `jetd`
could report: nothing is installed, or `status` failed.

| Owner | Other facts | Plan |
| --- | --- | --- |
| held, `homebrew` | any | Keep. App updates are off |
| held, `development` | any | Keep |
| held, `gui` | bundled release newer than `current` | Stage and activate. systemd brings `jetd` back (`Restart=always`); when it does not within 15 s (the unit does not supervise that daemon, or hit its start limit), `systemctl --user reset-failed` and `start jetd.service`. In autostart mode the app starts it |
| held, `gui` | otherwise | Keep. With systemd and no autostart file, write and enable the unit for later logins. Never start or restart |
| held, metadata missing or unknown | any | Keep, channel unknown |
| unknown, socket answers | any | Keep, channel unknown |
| free or unknown | `current` exists with a GUI unit or autostart file | Update first when the bundle is newer, then start: `systemctl --user reset-failed` and `start jetd.service`, or a detached `current/jetd serve --channel gui` |
| free or unknown | trusted Homebrew keg | `<brew> services start apexgang/tap/jet` |
| free or unknown | bundled payload | Install: stage and activate (skipped when an existing `current` is as new), then with systemd write the unit, `daemon-reload` and `enable --now`; without it write the autostart file and start a detached `jetd` for this session |
| free or unknown | nothing | Not installed |

Rules around the table:

- "Newer" is a semver comparison for the same target. A bundled release
  equal to `previous` never counts, so the next launch cannot undo a
  reviewed rollback. `current` is never downgraded, and nothing is staged or
  activated for another channel or once a Homebrew keg is chosen.
- When `status` failed and nothing answers on the socket, the pass changes
  nothing and reports the error. Acting without knowing the owner could
  start a second manager.
- When the update before a start fails (`service.update_refused`,
  `service.install_failed`, `service.payload_invalid`), the pass still
  starts the old `current` and keeps the error in the view. It skips that
  start when `jetd core` reports that another daemon took the Plane
  meanwhile (`service.channel_owned`, `service.owner_unknown`,
  `service.drain_timeout`).
- After any start the pass waits up to 15 s for the socket, then observes
  again. Each socket probe is bounded at 5 s, so a daemon that accepts and
  never answers counts as unreachable.
- When `jetd core activate` or `rollback` exits 4 (`service.drain_timeout`),
  it has already sent the daemon `SIGTERM` and `current` did not move. The
  pass then waits up to 60 s: once the Plane is free it starts the
  unchanged `current` (a detached `jetd`, or `reset-failed` and `start`
  under systemd); a daemon that answers again is left alone. The view keeps
  `service.drain_timeout`, whose copy now says the version was not changed
  rather than "nothing was changed".

**The payload.** The release overlay bundles the unmodified release archive
`jet-core-<version>-<label>.tar.gz` as the resource `jet-core.tar.gz`,
installed at `usr/lib/Jet/jet-core.tar.gz`. It must stay an archive:
linuxdeploy rewrites the rpath of every ELF file under an AppImage's
`usr/lib`, which would break the manifest digests `jetd core stage` checks.
A pass unpacks it with the system `tar --no-same-owner --no-same-permissions`
into a fresh 0700 directory under
`~/.cache/me.heeka.jet-tauri/local-service/`. It then requires exactly one
top-level directory, a `manifest.json` of at most 64 KiB whose version is
the app's version and whose target is this build's, and a regular `jetd`.
`jetd core stage --payload <dir>` and `jetd core activate --version <v>` run
from that unpacked `jetd`. The directory goes when the pass ends, and the
next pass clears any that a crash left. Development builds carry no payload
and end in "not installed" unless a core is already there.

**Service managers.** The unit and the autostart entry are the packaging
templates themselves, `include_str!` of `.github/packaging/linux/jetd.service`
and `jetd-autostart.desktop`. The unit gained
`ConditionFileIsExecutable=%h/.jet/core/current/jetd`, so a missing core no
longer restarts and fails every two seconds, and
`Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin`, because
Harness CLIs such as `claude` usually live in `~/.local/bin`.
`KillMode=process` and `Restart=always` are unchanged. systemd counts as
available when `systemctl --user show-environment` succeeds. `systemctl` and
`tar` are looked up in `/usr/bin` and `/bin`, then in every absolute
`$PATH` directory (NixOS and Guix profiles). Each candidate is
canonicalized and must be a regular executable file owned by root or this
user. Without a `tar` the pass fails with `service.tar_missing` rather
than `service.payload_invalid`. A unit left by
an older build is rewritten only when this build's template differs. A
detached `jetd` gets the unit's `PATH` and none of the AppImage runtime's
variables (`APPDIR`, `APPIMAGE`, `ARGV0`, `OWD`, or `PATH` entries inside the
mount).

**Homebrew.** The app looks for `<prefix>/opt/jet/bin/jetd` and
`<prefix>/bin/brew` under `$HOMEBREW_PREFIX` when it is absolute, then
`/home/linuxbrew/.linuxbrew`, then `~/.linuxbrew`. Both must canonicalize to
regular executable files owned by this user or root. homebrew/core's
unrelated `jet` (go-jet) links the same `opt/jet` but ships only `bin/jet`,
so it is never taken for Jet. `homebrew.rs` tests that, and `tests.rs` shows
that with go-jet installed the app installs its bundled core and never calls
`brew`. The app always names the formula in full.

**The provisioning lock.** Passes run one at a time in the process and,
through an advisory `File::try_lock` on `local-service.lock` in the app data
directory, across app instances, because `jetd core` uses fixed temporary
names. A pass waits up to 60 s for another instance, then reports
`service.busy`. A directory or an unwritable file at that path fails every
pass with `service.lock_unavailable`. Other limits: `status` 20 s, `stage`,
`activate`, `rollback` and extraction 120 s each, one `systemctl --user` call
30 s, `brew services start` 120 s.

**AppImage launcher entry.** deb and rpm install their own `.desktop` file.
An AppImage has none, so when the binary's bundle type is AppImage and
`$APPIMAGE` is an absolute regular file, the app writes
`$XDG_DATA_HOME/applications/me.heeka.jet-tauri.desktop` and the 128 px icon
`$XDG_DATA_HOME/icons/hicolor/128x128/apps/me.heeka.jet-tauri.png` at every
launch, only when their content differs. `Exec` quotes the canonical path as
the Desktop Entry Specification says. `TryExec` names the same file, so a
desktop hides the entry once the AppImage is gone. The cask installs the
AppImage at the version-less `~/Applications/Jet.AppImage`, so the entry
stays valid across `brew upgrade`.

**Setup.** While a pass works, Setup shows "Checking the Jet service on this
computer…", "Setting up the Jet service on this computer…", "Updating the
Jet service on this computer…" or "Starting the Jet service…" in a polite
live region instead of the generic offline copy. When the service
failed, stopped or is not installed, Setup names the problem and offers
Repair, or Check again when there is nothing to install. The Homebrew
failure copy names `brew services start apexgang/tap/jet`. Setup reads the
local Plane again when its feed reconnects (`connected` did not refresh it
before), when the feed opens after a failed startup read, and when the
service reaches running. After a first install it says "The Jet service
0.2.0 is set up on this computer."

**Settings › Safety and system › Versions** has a "Jet service on this
computer" block: Managed by ("Managed by this app", "Managed by Homebrew",
"Development build"), how it starts at login, status, the version, the
running version when it differs, the previous version, the version included
with this app, Repair or Check again, and "Roll back to <previous>…". A
failed update stays visible there while the old daemon keeps serving.

**Reviewed rollback.** `prepare_local_service_rollback` reads a fresh status
and, for a GUI-managed core with a `previous`, issues a one-use native review
token for that exact current and previous pair, valid for 10 minutes. The
dialog says the service stops and starts again with the previous version,
that running tasks keep running, and that the current version stays
installed. `execute_local_service_rollback` checks the pair again
(`service.review_stale` when it moved), runs `jetd core rollback`, and
brings the daemon back through systemd or a detached start.

**Commands.** `load_local_service`, `watch_local_service` (one Tauri Channel
per window) and `repair_local_service` in both windows;
`prepare_local_service_rollback` and `execute_local_service_rollback` in
Settings only. Launch runs the launcher refresh, one pass and the automatic
update check in a task spawned from `.setup()`, which never waits for it.

**Stable codes**, each with fixed copy: `service.channel_owned`,
`service.drain_timeout`, `service.install_failed`, `service.start_timeout`,
`service.start_failed`, `service.systemd_unavailable`,
`service.homebrew_start_failed`, `service.payload_invalid`,
`service.status_failed`, `service.owner_unknown`, `service.update_refused`,
`service.tar_missing`,
`service.rollback_unavailable`, `service.rollback_failed`, `service.busy`,
`service.lock_unavailable`, `service.review_expired`, `service.review_stale`.
`jetd core` exit 3 maps to a refusal code, exit 4 to
`service.drain_timeout`.

### App updates (§B)

- `tauri-plugin-updater` 2.12, Rust only with `rustls-tls`, registered only
  when the configuration has `plugins.updater` with a public key and at
  least one endpoint. The webview has no updater permission. Five Settings
  commands are the whole surface: `load_app_update`, `watch_app_update`,
  `check_app_update`, `install_app_update`, `restart_after_update`.
- Updates are off, with the reason shown, in this order of precedence: a
  build without updater configuration ("development build", which covers
  every development build and every unsigned pull-request or test bundle);
  a binary whose bundle type is not deb, rpm or AppImage, or an AppImage
  binary without `$APPIMAGE` (an extracted `squashfs-root`); the `jet-app`
  cask's AppImage (`$APPIMAGE` canonicalizes under a Homebrew prefix, or to
  `~/Applications/Jet.AppImage` while `<prefix>/Caskroom/jet-app` exists);
  a local service on the `homebrew` channel; and `service_unknown` while no
  settled service view names a channel ("Jet is still checking who manages
  it on this computer"). Unknown covers the launch pass before it settles,
  a first pass that starts `brew services`, and a daemon whose owner
  metadata is missing. A view that is still working keeps the channel of
  the settled view before it, so a Repair does not flicker updates off.
  "Nothing installed" counts as known. The checks follow the service view
  live. A download already under way, or an installed update waiting for
  its restart, still finishes.
- "Restart Jet" goes through Tauri's restart, which launches `$APPIMAGE`
  when it is set. A deb or rpm binary therefore clears `APPIMAGE` and
  `APPDIR`, which it can only have inherited, first thing in `run`, before
  Tauri reads them.
- Install downloads the bundle (30 minute limit). The plugin verifies its
  minisign signature against the configured key. `requireSignedVersion`
  rejects a signature whose `version:` field does not match the announced
  version, so a tampered `latest.json` cannot pair a newer version number
  with an older signed bundle. An AppImage is replaced in place. deb and rpm
  go through the plugin's privilege chain: pkexec, then a zenity or kdialog
  password for `sudo -S`, then plain `sudo`. "Restart Jet…" asks first
  ("Every Jet window closes and opens again.").
- One automatic check runs 10 s after the launch provisioning pass when the
  device preference "Check for updates automatically" is on, read both
  before and after the delay, so turning it off in the first 10 s stops it. It is on by
  default and stored as `checkForUpdates` in `desktop-preferences.json`.
  "Check for updates" in Settings always checks. See the privacy inventory
  for what a check sends. `set_desktop_preferences` now merges a change of
  one or both fields under its write lock, so a window with an old copy of
  "Reopen the last task" cannot undo it.
- Every Settings window follows the state through its own watcher, so a
  window opened during an install, or while the automatic check runs, sees
  it progress and finish.
- `LocalServiceView` and `AppUpdateView` carry a `revision` that only goes
  up (the update view's is the sum of its own change count and the service
  view's revision). Each window drops a view older than the one it shows,
  so a Repair or Check reply that crosses a newer pushed view no longer
  overwrites it.
- Stable codes: `update.offline`, `update.release_unavailable` (any non-2xx
  answer, retryable), `update.release_invalid` (also a `latest.json` body
  that is not JSON, which the plugin reports as a decode error of its HTTP
  client and used to show as offline), `update.check_failed`,
  `update.download_failed`, `update.signature_invalid`,
  `update.package_install_failed`, `update.install_failed`,
  `update.disabled`, `update.busy`, `update.not_available`,
  `update.not_ready`. The plugin cannot tell a canceled password prompt
  from a refused package, so both are `update.package_install_failed`.
- Signing is detached from the build. `JET_RELEASE_SIGN=true just
  release-bundle` keeps the updater endpoint and public key in the app but
  runs with the private key removed from its environment. `just
  release-sign` then runs `tauri signer sign --app-version <v>` on each
  bundle, which binds the file name and the version into the signature's
  trusted comment. In CI the `bundle` job references no secret; see
  "Release and Homebrew".

### Packaging (§C)

- The app is 0.2.0 in `tauri.conf.json`, `src-tauri/Cargo.toml` and its
  `Cargo.lock` entry, and `package.json` (ADR-0053). `just version-check`,
  the first step of `just check`, fails unless all four equal the core
  workspace version, and refuses a `version` key in the release overlay.
  `package.json` and `Cargo.toml` are Apache-2.0.
- Bundle metadata: publisher, copyright, category `DeveloperTool`, licence,
  homepage, short and long descriptions, targets `deb`, `rpm`, `appimage`,
  and `Recommends: git` for deb and rpm.
- `src-tauri/tauri.release.conf.json` maps `resources/jet-core.tar.gz` to
  the resource and sets `plugins.updater`: the public key (key ID
  `B530A38E9C125B31`), the endpoint
  `https://github.com/apexgang/jet/releases/latest/download/latest.json`,
  and `requireSignedVersion: true`. `src-tauri/resources/` is git-ignored.
- `just release-bundle <payload>` checks the payload archive (name, one
  top-level directory, manifest version and target, executable digests),
  copies it into `src-tauri/resources/`, removes old bundles so no stale
  `.sig` survives, and builds deb, rpm and AppImage. A release build
  (`JET_RELEASE_SIGN=true` or `1`) keeps `plugins.updater`. Anything else
  merges `"plugins":{"updater":null}`, so pull-request and test bundles
  carry no endpoint or key and never contact github.com. linuxdeploy runs
  with `NO_STRIP=1`, because its bundled `strip` cannot read `.relr.dyn` in
  current distribution libraries.
- `[profile.release]`: `codegen-units = 1`, `lto = true`, `opt-level = "s"`,
  `strip = true`. `panic` stays `unwind`: the shell turns a panicked
  `spawn_blocking` task into `PublicError::internal()` through its
  `JoinError`, and mutex poisoning relies on unwinding too.
- `CARGO_ENCODED_RUSTFLAGS` adds `--remap-path-prefix` for the repository
  (`/jet`), `CARGO_HOME` (`/cargo`) and the target directory (`/target`), so
  panic locations in the app binary no longer name the build user.
- The template logo is gone. The icons are the approved Wing J flat app icon
  (`docs/brand/jet-app-icon.png`), regenerated into the existing file names with
  `bun run tauri icon ../../docs/brand/jet-app-icon.png`.
- `just release-verify [--payload <archive>] [--release]
  [--require-signatures]` reads ar, tar, cpio and rpm itself and opens the
  AppImage with `--appimage-extract`. It checks the expected bundle names,
  that `usr/lib/Jet/jet-core.tar.gz` is byte-identical to the payload in all
  three, that each app binary carries its own bundle-type marker (what
  `bundle_type()` reads), no build paths in the app binary, the updater
  endpoint present in a release build and absent otherwise, no source maps
  or `fixtures/desktop` files, clean webview assets, and updater signatures
  that verify against `plugins.updater.pubkey` and name their file and the
  app version. It prints sizes and SHA-256s. `just release-check-signatures
  <dir>` checks only the signatures, without opening a bundle.

### Release and Homebrew (§D)

`release.yml` ("Release core executables and the Linux desktop app") runs on
every `v*` tag:

1. `validate` requires the tag to equal the workspace and app versions, the
   updater key in `tauri.release.conf.json`, and both updater secrets for
   every tag, prereleases included.
2. `package` builds and gates the core payload for three labels, as before.
3. `desktop-linux.yml`: the `bundle` job builds the x86_64 and aarch64
   bundles on `ubuntu-24.04` and `ubuntu-24.04-arm`, the core's glibc floor,
   and references no secret, because it runs the frontend dependencies,
   crate build scripts and the linuxdeploy tools the bundler downloads from
   mutable URLs. The `sign` job runs on a fresh runner that builds nothing:
   it checks out the same commit, downloads the unsigned bundles, runs `bun
   install --frozen-lockfile --ignore-scripts`, and gives the key to one
   step, `just release-sign`, then runs `just release-check-signatures`.
4. `homebrew-check.yml` and `desktop-e2e.yml` both need `desktop-linux`.
   `homebrew-check` renders the formula and cask for x86_64 with `file://`
   URLs, runs `brew style` and `brew audit --strict` (for the formula with
   `--except=version`, see below), adds a copy of the Swift app's `jet` cask
   as the real tap holds it, installs with the documented command, runs
   `jetd` under `brew services` in a systemd user session and checks the
   `homebrew` channel, then uninstalls while `jetd` runs and requires the
   left-over unit to stay inactive. `desktop-e2e` drives the signed x86_64
   `.deb` (see Verification).
5. `publish` needs all of them. `release_assets.py` checks the exact core
   and desktop sets, verifies every `.sig` with OpenSSL (Ed25519 over the
   BLAKE2b-512 digest, and the global signature over the trusted comment)
   against the pinned key, and writes `jet.rb`, `jet-app.rb`, `latest.json`
   and `SHA256SUMS`. It uploads everything to a draft, then publishes; a
   stable tag becomes Latest, a prerelease never does.
6. `homebrew`, for stable tags only, runs `update-homebrew.sh`.

`latest.json` has `version`, `pub_date` (the tagged commit's date in UTC, so
a rerun writes the same file) and six platforms,
`linux-{x86_64,aarch64}-{appimage,deb,rpm}`, each with the full `.sig`
content and the URL on the release. `SHA256SUMS` covers all 21 published
assets: six core archives, six bundles, six signatures, both Homebrew files
and `latest.json`.

**The `jet` formula** stays `apexgang/tap/jet`, rendered from
`.github/packaging/homebrew/jet.rb.in`, and is shared with the Swift
release. That release publishes a macOS-only `Formula/jet.rb` from the same
template when the tap has none or an older core version, and it owns
`Casks/jet.rb`, the macOS app. The tagged release writes the full macOS and
Linux formula at the same or a newer version and never touches
`Casks/jet.rb`. The template keeps `class Jet` and its `version "@VERSION@"`
line, which the Swift scripts parse. Homebrew's audit calls that line
redundant with the URL, hence `--except=version`. The generated systemd unit
gained `ConditionFileIsExecutable=#{opt_bin}/jetd`: uninstalling the cask
autoremoves the formula but leaves the unit `brew services` installed, and
the condition turns its next restart into a skipped start. A `caveats` block
names `brew services start apexgang/tap/jet`.

**The `jet-app` cask** (`jet-app.rb.in`) is Linux-only: `depends_on :linux`,
`depends_on formula: "apexgang/tap/jet"` (never the bare name, which
homebrew/core's go-jet would satisfy), `arch arm: "aarch64", intel:
"amd64"`, per-arch `sha256`, and `app_image` with the version-less target
`Jet.AppImage`. It needs Homebrew 6.0 or later. `livecheck` uses
`github_latest`, which relies on stable core releases being published with
`--latest` and Swift releases with `--latest=false` (see Remaining work).
There is no `auto_updates`: Homebrew updates it, and the
app's updater is off on that channel. The launcher entry, the icon and the
three `me.heeka.jet-tauri` directories under `~/.cache`, `~/.config` and
`~/.local/share` are in `zap trash:`, not `uninstall`, because `brew
upgrade` runs the old version's uninstall stanza. `~/.jet` is never zapped.
Install both names together, since Homebrew 6 trusts only the tap items
named on the command line:

```sh
brew install apexgang/tap/jet apexgang/tap/jet-app
```

or run `brew trust apexgang/tap` first.

`update-homebrew.sh` writes `Formula/jet.rb` and `Casks/jet-app.rb` in one
commit. It compares the tag with the highest published stable `vX.Y.Z`
release, never downgrades, skips both files when either is newer in the tap,
replaces the Swift release's macOS-only formula at an equal version, and
rebases onto concurrent tap commits before it pushes, failing if either file
changed under it.

**Release order.** The Swift release builds with the core version on `main`
and runs for every `apps/jet/` change. After a version bump it leaves the tap
with a macOS-only formula at the new version, and Linux installs fail until
that version's tag updates the tap. `docs/core-distribution.md` states the
rule: push the `v*` tag at the version-bump commit and let its release
finish before an `apps/jet/` change merges.

**`packaging.yml`** rehearses the x86_64 half of a release on pull requests
that touch packaging inputs, the journey, its timing marks, the provisioning
code or the app's dependency and toolchain files, and on demand. It builds
and gates the payload, bundles the app unsigned, runs `desktop-e2e` on the
unsigned `.deb`, and runs `homebrew-check`. It uses no secret and is not a
required check. When only the size gate fails, the later jobs still run on
the uploaded payload, and the run stays red.

`.github/dependabot.yml` gains `cargo` for `/apps/jet-tauri/src-tauri`, the
Tauri half of #177.

### Hardening (§E)

**Command IDs (D1 to D3).** A Plane stores a Command's durable answer under
its ID, refusals included, so reusing the ID replays that answer (ADR-0093).
`command_ids.rs` keeps an ID only while the outcome is uncertain (outcome
unknown, a lost transport, an expired deadline) and drops it on any definite
answer. `PendingCommands` ties a kept ID to its target and its exact
request, so it is never sent for different work.

- D1, `run_control.rs`. Interrupt is keyed by the active Turn from the
  pre-check, Stop by Run. `run.not_controllable` forgets both kept IDs for
  that Run and `run.no_active_turn` forgets the Interrupt's, so a later
  Turn gets a new ID. Withdraw and approval retries follow the same rule.
- D2, `conversations.rs`. `create_conversation`, `start_run` and
  `submit_turn` take a required `attempt` UUID, one per composer Send
  (`conversation.attempt_invalid` when malformed). The webview keeps it only
  across unchanged retries and clears it on success or any draft edit, so a
  later Send of the same text is a new request. A kept attempt is bound to
  the kind (Run start or Turn), task, Plane and Craft it was first sent as,
  and a retry resends exactly that, even when the task now shows a live Run
  (or none). A Send of the same draft to another task gets a new attempt.
  The shell refuses an attempt kept for another kind or task with
  `conversation.attempt_conflict` before contacting the Plane, so neither
  side can send the prompt twice.
- Kept IDs are capped at 256 per kind of request. When the map is full,
  the oldest ID older than the Command deadline (180 s) is evicted and its
  retry becomes a new request; only while all 256 are younger is a new
  request refused with `client.too_many_pending_commands`.
- D3, `setup.rs`. While a bind ID is kept, the next bind reads the Plane's
  account bindings first and drops the ID when that provider has none.
  Project register and remove grants get a fresh ID after a definite
  refusal. A failure after an attempt that may have been sent, a refused
  reconnect handshake included, is `ConnectError::Unconfirmed` and never
  definite.
- Work-panel terminal open and close and file review follow the same rule.
  A save's late answer settles only its own Command ID, so it no longer
  clears a newer pending edit of the same file.

**Stuck edits (D4).** A pending edit records its expected revision, and a
retry resends the exact body. A definite refusal, `user_edit.stale_revision`
included, or a reload that shows a newer revision ends it, so the user is
neither stuck nor forced to overwrite. `save_work_file` and
`submit_file_review` connect before they record anything, so a save that
never reached the Plane does not lock the file.

**Client ID recovery (D5).** `client-id` reads stop at 64 bytes. An
unreadable file becomes `client-id.invalid`. The app then recovers the
identity from the attributes of the Secret Service item that holds this
computer's pairing key, when exactly one exists: no unlock, no secret read,
a 5 s limit, and only the lowercase hyphenated form it writes. A locked
legacy gnome-keyring shows 32-hex MD5 hashes of attribute values; those are
refused. Otherwise it creates a new identity. Temporary files get unique
names, so a stale one cannot block the save. The Planes panel says which of
`identity_recovered`, `identity_replaced` or `identity_unsaved` happened.
Loading the identity can no longer fail launch.

**Plane registry (D8).** A `planes.json` from a newer Jet, or one this launch
cannot read (a permission error, for example), stays untouched. A FIFO,
socket or device in the place of `planes.json` or any other app data file
is opened non-blocking and treated as damaged (`local_store::open_regular`);
it used to block launch in the read. `planes.json` is then set aside with
`registry_reset`, like a directory. The
registry is read-only for the launch, the Planes panel shows
`registry_newer` or `registry_unreadable`, and changes fail with
`plane.registry_read_only`. Add Plane checks this before any ssh or pairing
step. A directory in the file's place is still set aside, with
`registry_reset`.

**Deadlines (D6).** `deadline.rs` bounds every wait on a Plane:

| Wait | Deadline | On expiry |
| --- | --- | --- |
| Liveness: local connect and handshake, `PlaneClient::status`, the status read that ends a remote login, one event-feed page | 30 s | `transport.offline`, retryable. A feed reports Reconnecting and dials again; a remote login backs off |
| Query: any other read on an open connection | 90 s | `transport.offline`, retryable |
| Command | 180 s | `command.outcome_unknown`. The ID is kept, so a retry resends the same request |

`jetd` serves one connection's requests one at a time, in arrival order,
and a remote Plane's Queries, Commands and event feed share one ssh
session. A request's deadline therefore starts when it reaches the head of
its connection's queue (`RequestQueue` in `deadline.rs`), not when it is
sent: a feed page or status read queued behind a slow but healthy Query
waits for it instead of expiring at 30 s and dropping the session, and
with it the Query. Only the head can expire. When it does, the peer is
hung: every request queued behind it expires at once and the session is
dropped once. A hung peer is detected within the head request's own
deadline, so up to 90 s (a Query) or 180 s (a Command) rather than 30 s
when one of those is at the head.

The core bounds each of its own network steps at 30 s, and Craft discovery
makes two in sequence, so Queries get 90 s. Commands that work before they
answer, such as preparing a Craft installation, get twice that. After any
expired wait the connection fails its later requests at once, local or
remote, and an expired remote session is dropped so the next request logs in
again. Every non-test Plane request goes through `Connection::query` or
`Connection::command`. The compiler does not enforce that, because
`Connection` derefs to `jet_client::Client`; a grep against every public
`jet-client` function checked it. Not bounded here: the signed remote
handshake and the pairing exchange (15 s inside `jet-client`), the keyring
unlock prompt (60 s, `keystore.rs`), and attached-terminal streams.

**Tests.** `app_data_tests.rs` damages every file in the app data directory
six ways (empty, truncated, oversized, garbage, a directory in its place, a
newer version): `client-id`, `planes.json`, `last-conversation`,
`desktop-preferences.json`, `notification-preferences-v2.json`,
`settings-window.json`, `shell-presentation.json`, `window-geometry.json`
and `local-service.lock`. The app state `lib.rs` builds still loads, and each
file is reset, set aside or kept as its module specifies. For the lock, a
directory fails the pass with `service.lock_unavailable` and every other
damage lets the pass run with the bytes kept; a separate test covers a
read-only lock file and app data directory. Revision conflicts gained a Rust
test in `errors.rs` that a conflict crosses with only its safe state, a
run-control conflict test, and `revision-conflict.test.ts` for
`applyRevisionConflict`. That behaviour already worked, so these pass
before and after. The implementation rounds report that each defect test
failed with its fix reverted, and that the six deadline tests hung without
the deadline.

New codes and notices: `command.outcome_unknown` produced by the shell,
`plane.registry_read_only`, `conversation.attempt_invalid`, the notices
`identity_recovered`, `identity_replaced`, `identity_unsaved`,
`registry_newer` and `registry_unreadable`, plus the `service.*` and
`update.*` codes above.

### Plane restarts after a redial (lands separately)

This change is not on `w4/docs` at `fb346e1`; it lands as its own change.
After a drop, the native feed reads the Plane's status again when it dials
again and sends `connected` with that snapshot before `resumed`. The
registry records the new status first. The webview's `connected` handling
then runs its daemon-restart path (`planeRestarted`) when `daemonStarts`
changed, so after a `jetd` restart or a core activation the Planes list,
Setup and Settings › Versions show the new core version without a manual
refresh. Recent is read once per redial. The protocol knowledge that Plane
detail and Versions show (the negotiated minor) resets when `daemon_starts`
or `core_version` changes, because another run may be another core.

Without it, this branch handles a redial through `resumed` only. That marks
`local-plane-connected` and reads the registry, Setup, Jet Trash and the
selected task again, but it cannot tell that `jetd` restarted, so health
keeps its last summary and version displays stay stale until the next
`connected`.

### Review fixes (2026-09-25)

A review of this branch found the defects below, and each fix has a test.
The remote queued-session test was run against the old deadline timing and
failed; the new Vitest cases failed against the old webview code. The other
Rust tests exercise paths the old code did not have or asserted the
opposite of, and were not re-run against it.

- Remote Planes: one expired request no longer drops the shared ssh
  session under healthy work queued with it (see Deadlines, D6).
- Composer retries resend the request an attempt was first sent as (D2).
- App updates stay off while the service channel is unknown, for the
  cask's AppImage, and for an AppImage binary without `$APPIMAGE`; "Restart
  Jet" cannot launch an inherited `$APPIMAGE`; the automatic check re-reads
  its preference after the delay; an undecodable `latest.json` is
  `update.release_invalid` (App updates).
- A drained daemon that systemd does not bring back is started through the
  unit, and a daemon that outlived a drain timeout is started again once
  it leaves (Automatic core installation).
- FIFOs in the app data directory no longer block launch (D8). Uncertain
  Command IDs can be evicted after 180 s. `systemctl` and `tar` are found
  on `$PATH`, and a missing `tar` has its own code.
- Views carry a `revision`, so stale Repair and Check replies are dropped.
- The redial snapshot: the native feed can send `reconnecting`, `failed` or
  a redial's `connected` before `open_plane_feed` answers. The webview no
  longer applies the older opening snapshot over them, which had brought
  back an old `daemonStarts` and a false restart.
- Settings › Versions: the App updates live region announces phase
  changes only ("Downloading Jet X…" once; the percentage and the progress
  bar sit outside it). One action button changes through Check, Install
  and Restart and keeps focus, marked busy rather than disabled while work
  runs. When a focused control disappears, focus moves to the first control
  left in the block or to its heading (`keep-focus.ts`). A failed service
  pass is `role="alert"`, and Repair stays in place as "Repairing…" during
  its own pass.

## Boundaries

- No Jet protocol call was added. Provisioning uses `jetd core` locally,
  and app updates use the Tauri updater and GitHub.
- Every process and file action is native, with fixed argument lists from
  canonical, trusted `systemctl`, `tar` and `brew` paths and the chosen
  `jetd`. The
  webview passes no path, command or version.
- The main window can read, watch and repair the service. Only the Settings
  window can roll it back or touch updates.
- macOS is out of scope. This change edits nothing under `apps/jet` or the
  Swift release scripts, and the macOS column of the parity matrix is not
  assessed for the Wave 4 rows.

## Privacy inventory

### Files

The app data directory is `$XDG_DATA_HOME/me.heeka.jet-tauri`, by default
`~/.local/share/me.heeka.jet-tauri`, mode 0700, and its files are 0600.
New in Wave 4: `local-service.lock`, `client-id.invalid`,
and the `checkForUpdates` field.

| File | Holds |
| --- | --- |
| `client-id` | This installation's client identity, a UUID. Not a credential |
| `client-id.invalid` | An unreadable `client-id`, set aside |
| `planes.json` | The Plane registry: each remote Plane's ssh destination, Plane identity, credential kind and date added |
| `planes.json.invalid` | An unreadable registry, set aside |
| `last-conversation` | The last selected task's ID and its Plane |
| `desktop-preferences.json` | "Reopen the last task" and "Check for updates automatically" |
| `notification-preferences-v2.json` | Notification toggles and muted Planes. An older `notification-preferences.json` is read when v2 is missing |
| `settings-window.json` | The last Settings pane |
| `shell-presentation.json`, `window-geometry.json` | Window layout, sizes and the maximized flag |
| `local-service.lock` | Nothing; the provisioning lock |

Tauri points WebKitGTK's website data and cache at the same directory, and
wry keeps a `cookies` file there. The app keeps nothing in browser storage.
Which files WebKitGTK actually creates was not observed.

Outside the app data directory:

| Path | Written by | When |
| --- | --- | --- |
| `~/.cache/me.heeka.jet-tauri/local-service/payload-<id>/` | The app | During a pass that needs the bundled payload; removed when it ends |
| `~/.jet/core/versions/<v>/`, `current`, `previous` | `jetd core stage`, `activate` and `rollback`, run by the app | Install, update, rollback |
| The rest of `~/.jet` | `jetd` itself | The core's state (`docs/core-distribution.md`) |
| `$XDG_CONFIG_HOME/systemd/user/jetd.service`, and the `default.target.wants` link `systemctl --user enable` makes | The app and systemd | Install with a systemd user manager; rewritten when this build's template differs |
| `$XDG_CONFIG_HOME/autostart/jetd.desktop` | The app | Install without a systemd user manager |
| `$XDG_DATA_HOME/applications/me.heeka.jet-tauri.desktop` and `$XDG_DATA_HOME/icons/hicolor/128x128/apps/me.heeka.jet-tauri.png` | The app | Every AppImage launch, when the content differs. The entry holds the AppImage's absolute path |
| Homebrew's `jet-homebrew` user unit | Homebrew | When the app runs `brew services start apexgang/tap/jet` |
| `tauri_deb_update*`, `tauri_rpm_update*` or `tauri_current_app*` temporary directories in `$TMPDIR`, else `~/.cache` or next to the app | The updater plugin | During an install; removed after |
| The AppImage file, or the system package through pkexec or `sudo` | The updater plugin | When the user installs an update |
| An audit evidence file and its partial file | The app | Only when the user exports audit evidence to a path they chose (Wave 3.3) |

### Secret Service

- "Jet client identity", one item per client identity, with the attributes
  `application=me.heeka.jet-tauri`, `jet-purpose=client-identity`,
  `jet-client-id=<UUID>` and `jet-key-algorithm=ed25519`. The secret is this
  computer's Ed25519 pairing key seed. Wave 3.1 writes it; D5 reads only its
  attributes.
- "Jet secure storage check", created, read back and deleted by the probe
  before pairing.

### Network

| Contact | When | What it sends |
| --- | --- | --- |
| `GET https://github.com/apexgang/jet/releases/latest/download/latest.json`, following GitHub's redirect | Release builds with updates on: once, 10 s after the launch pass, when "Check for updates automatically" is on (the default), and on "Check for updates" | The computer's address and the time, with the user agent `tauri-plugin-updater/2.12.0` and `Accept: application/json`. The URL carries no Jet version, client ID or Plane data |
| The bundle URL `latest.json` names, on the same release | "Install Jet <version>" | Which release this computer downloads |
| The system `ssh` to each remote Plane the user added | While that Plane is connected (Wave 3.1) | ssh's own traffic to a host the user chose |

The app contacts nothing else. The webview's CSP allows `connect-src ipc:
http://ipc.localhost` only, the app has no telemetry, and local work uses
the socket `~/.jet/runtime/jetd.sock` and D-Bus (systemd, Secret Service,
notifications). `jetd` makes its own contacts, such as GitHub for Git
delivery, Extension catalogs and Craft discovery; those belong to the core.
Whether `brew services start` contacts the network is Homebrew's behaviour
and was not checked.

The request goes to a fixed URL with the updater plugin's own User-Agent
(`tauri-plugin-updater/<version>`), so it does not carry Jet's version; the
note under the Settings toggle says only that GitHub learns this computer's
address.

## Verification

Run for this record on 2026-09-24 at `fb346e1` (`w4/docs`), from
`apps/jet-tauri`:

```sh
bun install --frozen-lockfile
bun run test
RUSTUP_TOOLCHAIN=1.98.1 TMPDIR=<short directory> cargo test --locked --manifest-path src-tauri/Cargo.toml
bun tests/e2e/main.ts --dry-run --quiet
```

- Vitest 5.0.1: 638 tests in 53 files passed.
- Rust: 386 passed, 0 failed, 3 ignored. The ignored tests are the live
  ones that need a real Secret Service, a remote Plane sandbox and a built
  `jetd`. The Unix socket tests need a short `TMPDIR`, because a socket path
  must fit in `SUN_LEN`.
- The journey's dry run exited 0 with 52 checks. Only the optional "app tree
  includes the WebKit web and network processes" check failed, because the
  fake app is `sleep`.

Not re-run for this record: `bun run check`, `bun run build`, rustfmt,
clippy, `.github/tests`, actionlint and `gh actions-lock --verify-local`.
The last implementation rounds report them clean: svelte-check with 0
errors, clippy with `-D warnings`, 51 `.github` tests with a real portable
Ruby 3.4.5, actionlint 1.7.12 with shellcheck 0.11.0, and lockfile coverage
for every workflow. The parallel redial change reports 642 Vitest and 386
Rust tests in its own worktree; it was not run here.

### Local release bundles

Built on the Arch Linux development host for x86_64 during the
implementation rounds, not rebuilt for this record. The payload was
`jet-core-0.2.0-x86_64-unknown-linux-gnu.tar.gz`, 8,897,865 bytes, sha256
`8c081a1c…`, whose `jetd` is over its budget.

| Result | Value |
| --- | --- |
| Release binary, default profile | 32,387,080 bytes, 21,562,728 after `strip` |
| Release binary, new profile | 11,272,392 bytes |
| Final release build (`JET_RELEASE_SIGN=true`, key unset) | deb 14,949,920, rpm 14,951,418, AppImage 120,326,648 bytes |
| Payload inside each bundle | Byte-identical in all three (`release-verify`, and `cmp` on an earlier build) |
| `just release-sign` in a rehearsal of the sign job (fresh app tree, `--ignore-scripts`) | Three signatures of 424, 428 and 432 bytes; trusted comments name the file and `version:0.2.0`; `release-check-signatures` and `release-verify --require-signatures` pass; neither the key nor the password appears in any log |
| `release_assets.py` on the real signatures | Accepted. A one-byte change to the deb, or a `.sig` moved to another bundle, is refused |
| Unsigned build | No updater endpoint or public key in the binary |
| Build paths | No `/home/` path in the app binaries after the remap |
| Package metadata | deb `Recommends: git`; rpm `RECOMMENDNAME` git; the AppImage entry has `Categories=Development`; each binary carries only its own bundle-type marker |
| Negative checks | A tampered payload fails all three bundles; `--require-signatures` and `release-check-signatures` fail on unsigned bundles; `release-sign` without the key exits 2; a stray file in the signing directory is refused |

The AppImage bundles the build host's libraries, so its size on Ubuntu 24.04
will differ. The bundled app was never launched on this host: it would
provision the real `~/.jet`, systemd unit and keyring.

### Desktop journey on release artifacts

`apps/jet-tauri/tests/e2e/` with `.github/workflows/desktop-e2e.yml`, on a
fresh `ubuntu-24.04` runner: install the `.deb` with `apt-get`, enable a
lingering systemd user session, then drive the app through tauri-driver
2.0.6 and WebKitWebDriver under Xvfb, with `WEBKIT_DISABLE_DMABUF_RENDERER=1`
and `LIBGL_ALWAYS_SOFTWARE=1`. It asserts through the DOM, by role and
accessible name, and on the machine:

1. The cold launch provisions the service from the payload: Setup shows the
   local Plane connected on the bundled core with "set up on this
   computer", `~/.jet/core/current` exists, `jetd.service` is enabled and
   active, and `jetd core status` reports `gui` at the release version.
2. After 15 quiet seconds, CPU and PSS of the app tree and of `jetd` are
   sampled over 60 s.
3. Settings › Versions in its own window shows "Managed by this app", the
   release version and the bundled version. App updates show the updater's
   controls in a signed build and "development build" in an unsigned one.
4. `systemctl --user kill jetd.service`: the daemon restarts and the UI
   reconnects on its own.
5. Three quits and relaunches: the same connected state and nothing
   provisioned again. After the first, the same `current` and the same
   `jetd` process.

The launch, provisioning, idle and reconnect measurements go into
`desktop-e2e.json` and never gate; `docs/resource-budgets.md` has the
proposed ceilings. The journey has never run in CI, so there is no desktop
measurement. `just e2e-dry-run`, part of `just check`, runs it against a fake
driver, a scratch home and `sleep` processes.

### Review fixes, 2026-09-25

From `apps/jet-tauri` with `RUSTUP_TOOLCHAIN=1.98.1` and a short
`TMPDIR`: `just version-check`, `bun run check` (0 errors, 0 warnings),
`bun run test` (655 tests in 53 files), `bun run build`, `cargo fmt
--check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test
--locked` (412 passed, 3 ignored) and `just e2e-dry-run` all passed.
Nothing was run against real systemd, Homebrew, GitHub or a remote Plane.

## Remaining work and unverified items

Blocking a release:

- `jetd` is over its 12 MiB budget: 18.31 MiB at `0fde750` per #215.
  `release-check` fails, the `package` job fails, and nothing publishes.
  Getting under needs a release profile decision under ADR-0059.
- The desktop journey has never run on GitHub Actions. Dispatch
  `packaging.yml` before the first `v*` tag so its first real run does not
  gate `publish`. Known risks: WebKitWebDriver may not list the Settings
  window, WebKitGTK under Xvfb may not render, the WebKit sandbox may hide
  PSS, and the app may need a forced quit at session end.
- `homebrew-check` has never run. `brew style`, `brew audit --strict
  --except=version`, tap trust for the two-name install, the Swift cask
  next to the formula, `brew services` in a lingering session and the
  no-restart-loop check all rest on reading Homebrew's source. No live
  Homebrew install has been exercised anywhere.
- The Wave 4 jobs of `release.yml` and all of `packaging.yml` have never
  run. The aarch64 bundles have never been built; only CI builds them, and
  the bundle job's setup on `ubuntu-24.04-arm` (WebKitGTK packages, bun,
  the Rust toolchain) is untested.
- On 2026-09-24 the real tap holds a macOS-only `jet` 0.2.0 written by the
  Swift release, so Linux Homebrew installs fail until `v0.2.0` publishes.

Unverified on a real system:

- GitHub resolves the repository's latest release to `swift-v1.0.5` today,
  although the Swift release workflow passes `--latest=false`. While that
  holds, the updater endpoint and the cask's livecheck point at a Swift
  release, which has no `latest.json`, so a check would report
  `update.release_unavailable`. Whether a core release published with
  `--latest` keeps Latest across later Swift releases was not observed.
- Provisioning, repair and rollback against real systemd, XDG autostart,
  `brew` and `jetd core`: fakes only.
- An update check, download and install against GitHub, pkexec, dpkg or
  rpm: fakes only.
- The rpm and the AppImage were never launched; the journey drives only the
  x86_64 `.deb`. The AppImage launcher entry is covered by unit tests only.
- Keyring recovery (D5) against a live Secret Service or a locked legacy
  gnome-keyring: pure-function tests only.
- The identity and registry notices in a running window: Vitest only.
- Setup's `installing` phase is recorded, not required, by the journey,
  because a fast pass can finish before the page's watcher registers.

Known limits:

- Bundled Crafts are unusable after the core payload installs (#210).
- Payload trust rests on the bundle and its updater signature.
  `jetd core stage` checks only the manifest's own digests.
- A cask AppImage is recognised by its path: under a Homebrew prefix, or
  `~/Applications/Jet.AppImage` while the Caskroom holds `jet-app`. An
  AppImage the user copied to that path while the cask is installed is
  indistinguishable and does not update itself. How Homebrew 6's
  `app_image` artifact places the file (a move or a link into the
  Caskroom) was not observed.
- After a drain timeout the pass waits at most 60 s for the daemon to
  leave. A daemon still draining after that is not started again in that
  pass; the next launch or Repair starts the old `current`.
- A deb or rpm update from an app started in a terminal can end in the
  plugin's plain `sudo` prompt on that terminal and wait without limit, with
  the updater busy.
- `checking` is bounded but can last minutes in the worst case: 60 s for
  another instance's lock, 20 s status, 5 s probes, 120 s each for
  extraction, stage and activate, 15 s more for systemd to restart a
  drained daemon before the app starts the unit, and 60 s after a drain
  timeout.
- A root-owned or unwritable `local-service.lock`, for example after
  running the app with `sudo`, fails every pass with
  `service.lock_unavailable`, and the message does not name the file.
- Composer attempt IDs live in memory. A retry after a webview reload is a
  new request and can duplicate a create or send whose uncertain outcome was
  applied.
- A Command that runs past 180 s reports `command.outcome_unknown`; what
  `jetd` does with a duplicate of a Command still running was not checked.
  Half-open ssh links are caught only by these deadlines, within the
  deadline of whatever request is at the head of the session's queue.
- The request queue models `jetd`'s order from the shell's side. A request
  whose caller stops waiting leaves the queue at once, although `jetd`
  still serves it, so the next request's deadline can start early; and two
  requests started in the same instant may reach the wire in the other
  order. Either can end one wait early; neither was seen in tests.
- Add Plane stays enabled while the registry is read-only; it is refused at
  once.
- Launch still fails if Tauri cannot resolve the home or app data
  directory; D5 covers only the identity file.
- The linuxdeploy tools the bundler downloads come from unpinned URLs.
  The Tauri CLI's signer, pinned in `bun.lock`, is the one process that
  sees the updater key.
- The deb `Maintainer` field has no email, which lintian warns about.
- `scripts/release/*.ts` is outside svelte-check's `include`; `main.ts` was
  type-checked only by one-off `tsc` runs.
- The Settings copy about what an update check shares overstates it (see
  the privacy inventory).

Still open from earlier waves:

- The Wave 3.4 exceptions PE-1, PE-2, PE-5, PE-6, PE-7 and the deferral PD-1
  await the product owner. The Wave 3.4 manual native checklist has not run.
- macOS is not verified by this Tauri-only change.
