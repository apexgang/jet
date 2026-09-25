# CI and releases

This directory contains Jet's GitHub workflows. Their supporting scripts,
packaging files, tests, and issue-tracker instructions live elsewhere under
`.github/`. `packages/justfile` remains the entry point for local backend
commands.

## CI

Core checks retain Linux and macOS coverage, Clippy, migrations, the dependency
envelope, unit tests, integration tests, and doctests. The Tauri app job runs
the app's `just check` and `just audit` on Linux: contract freshness, Svelte and
TypeScript checks, frontend tests and build, and Rust formatting, Clippy and
tests against the locked `Cargo.lock`. It then audits npm and RustSec
advisories. Its accepted exceptions are reviewed in
[dependency advisories](../../apps/jet-tauri/docs/dependency-advisories.md).
Contract checks retain Rust, TypeScript, and Swift coverage. Each workflow first
compares the complete change against the PR base or the push's previous commit.
Unknown history runs all checks.

The main ruleset requires the `ubuntu-latest` and `macos-latest` checks. The
`required` gate in `core.yml` reports both once the core and Tauri jobs finish.
It passes only if change selection succeeded, every selected job passed, and
every other job was skipped. The jobs cannot report these names themselves: a
job skipped by its `if` never expands its matrix, so an edit outside
`packages/` would leave both checks absent and block the pull request. Edits
that select neither job, such as documentation or the Swift app, pass the gate
without running either suite. CodeQL compiles the Swift app on pull requests;
the Swift release workflow builds it after changes land on `main`.

Test runners append `ci/cargo.toml` to their Cargo configuration. Workspace
code compiles without optimization or LTO, with 256 codegen units. Dependencies
use optimization level 1 so cryptographic tests stay practical. Local build
settings and optimized release profiles are unchanged. Caches include workspace
crates, use separate keys for each build graph, and save even when tests fail.
A source timestamp is restored only if its SHA-256 matches the cached input;
changed inputs receive current timestamps so Cargo rebuilds them. This avoids rebuilding
unchanged workspace crates after every checkout.
Directory timestamps also account for added and removed tracked inputs, so
the store's migration watcher remains correct while reusing unchanged builds.
`just ci-test` uses nextest to schedule tests across binaries concurrently;
the bulk PTY and multi-daemon origin scenarios reserve the runner to avoid
contention-induced timeouts. These scenarios retain their original deadlines.
Cargo doctests run in a separate step even when a test fails. The local
`just test` command still uses Cargo's built-in runner.

The baseline is [run 35009475464](https://github.com/apexgang/jet/actions/runs/35009475464):
Linux test compilation took 9m 54s and macOS took 21m 01s. The whole macOS job
took 30m 23s. Compare the first cold run and a later cached run separately;
cache restore time and the test suite itself still contribute to elapsed time.

## Code scanning

`codeql.yml` is an advanced CodeQL setup covering GitHub Actions, JavaScript
and TypeScript, Python, Rust, and Swift on every pull request, every push to
`main`, and a weekly schedule. It replaces GitHub's default setup, which could
not analyze Swift: the Xcode project uses file system synchronized folders, so
the Swift autobuilder found no target with Swift sources, and code scanning
rejects CodeQL uploads from a workflow while default setup stays enabled. The
Swift job runs on macOS and compiles the GUI (`xcodebuild`, macOS SDK, signing
disabled) and the shared wire models (`just contracts-test-swift`) under the
CodeQL tracer. A test in `.github/tests/test_automation.py` fails when a Swift
source appears outside the roots that build step compiles, so a new Swift
target needs a matching build command before CodeQL sees it. Analyses keep
the `/language:<name>` categories default setup used, so existing alerts carry
over. Keep default setup disabled in the repository's code security settings;
re-enabling it silently rejects this workflow's uploads.

## Releases

1. Set the workspace version, and the desktop app's version in
   `apps/jet-tauri` to the same value (ADR-0053), then push the matching
   `vMAJOR.MINOR.PATCH` tag. `validate` refuses a tag that differs from
   either, and a release configuration without the updater public key.
2. The release workflow builds Linux x86_64, Linux ARM64, and universal macOS
   payloads, preserving the per-role profiles and enforcing the release envelope.
3. `desktop-linux.yml` bundles the Tauri app for each Linux payload as a deb,
   an rpm, and an AppImage on the same runner images, with the payload archive
   inside. A separate job on a fresh runner then signs every bundle for the
   updater.
4. `homebrew-check.yml` renders the formula and the cask against the x86_64
   artifacts, runs `brew style` and `brew audit --strict`, installs both from
   a local tap with the documented command next to a copy of the Swift app's
   `jet` cask, as the real tap holds it, runs the daemon under
   `brew services` in a systemd user session, and checks that uninstalling
   leaves no restarting service behind. The formula's audit skips only
   `version`: the Swift app's release parses the formula's `version` line,
   which Homebrew otherwise reports as redundant with the release URLs.
5. `desktop-e2e.yml` installs the signed x86_64 `.deb` on a fresh runner and
   drives the app through tauri-driver and WebKitWebDriver under Xvfb. The
   app must provision its service from the bundled payload, Settings ›
   Versions must show the service managed by the app, the UI must reconnect
   after `systemctl --user kill jetd.service`, and a relaunch must change
   nothing. The job uploads its launch, idle and reconnect measurements and
   screenshots as `jet-desktop-e2e-<label>` (see
   [resource budgets](../../docs/resource-budgets.md#desktop-linux)).
6. Once every gate passes, it uploads six core archives, six desktop bundles
   with their signatures, `jet.rb`, `jet-app.rb`, the updater's
   `latest.json`, and a `SHA256SUMS` covering all of them into a draft
   release, then publishes the complete release. `latest.json` embeds each
   signature file's contents and dates the release by its tagged commit.
   GitHub's Latest, which the updater and the `jet-app` cask's livecheck
   read, goes to the highest published stable `vMAJOR.MINOR.PATCH` release,
   so a patch for an older version publishes without taking it. The job
   fails if any other release holds Latest.
7. For a stable release, Ape Bonker commits `Formula/jet.rb`, the core
   formula for macOS and Linux, and `Casks/jet-app.rb`, the Linux desktop
   cask, of the highest published stable release to `apexgang/homebrew-tap`
   in one commit. That is usually the tagged release; for a patch to an
   older version it is the newer release. It replaces the macOS-only
   formula the Swift app's release may have written at the same core version,
   never touches that release's `Casks/jet.rb`, and rebases onto the Swift
   release's tap commits before it pushes.
   `brew install apexgang/tap/jet` installs the compiled executables and
   `brew services start apexgang/tap/jet` starts the daemon. On Linux,
   `brew install apexgang/tap/jet apexgang/tap/jet-app` also installs the
   desktop app. Name both, or run `brew trust apexgang/tap` first: Homebrew 6
   and later trust only the tap items named on the command line, and the cask
   cannot load its formula otherwise. Always use the full name:
   homebrew/core's unrelated `jet` owns the bare one.

The Actions secret `APE_BONKER_PRIVATE_KEY` holds Ape Bonker's private key.
`RELEASE_APP_CLIENT_ID` identifies the installed app; the workflow defaults to
the verified Ape Bonker client ID. `actions/create-github-app-token` restricts
the short-lived token to `contents:write` on `homebrew-tap` and revokes it at job
completion. The normal `GITHUB_TOKEN` publishes Jet releases. PR jobs receive
neither the private key nor the tap token.

`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` hold the
updater's minisign key. Every release tag needs both, prereleases included.
The `bundle` job of `desktop-linux.yml` runs the frontend dependencies, crate
build scripts, and the linuxdeploy tools the Tauri bundler downloads, so it
never receives them. Steps of one job share a runner, so code in the build
could change what a later step runs; the key therefore goes only to the
`sign` job on a fresh runner. That job checks out the same commit, downloads
the unsigned bundles, installs the locked frontend packages with
`--ignore-scripts`, and gives the key to one step, `just release-sign`, which
hands it to the Tauri CLI's signer alone. The build uses
`JET_RELEASE_SIGN=true`, which keeps the updater endpoint and public key in
the app. The signer binds each signature to the bundle's file name and the
release version, which the app's `requireSignedVersion` checks. The matching
public key is `plugins.updater.pubkey` in
`apps/jet-tauri/src-tauri/tauri.release.conf.json`; the `sign` job and
`release_assets.py` in the publish job both verify every signature against it
(OpenSSL checks the Ed25519 signatures for `release_assets.py`), so a
signature made with any other key, which Tauri itself only warns about, or a
bundle changed after signing is never published. Rotating the key means
changing both together, and installed apps accept only updates signed with
the key they shipped with.

Prerelease tags must also match the workspace version. They publish prerelease
assets without modifying the stable formula or cask, and the updater never
offers them because it reads the latest stable release. Rerun failed jobs to
retry a tap update. A rerun of the publish job for a published release
downloads no artifacts, which expire after seven days, keeps the published
assets intact, and only checks that the highest stable release holds Latest;
if it fails there, run `gh release edit <that tag> --latest`. The tap job
always downloads the published formula and cask, so rebuilt checksums cannot
replace the published ones. Dispatch the workflow against a version tag,
never a branch.

The tap jobs of all core releases share one concurrency group, and GitHub
keeps only one pending job per group: a newer pending job cancels an older
one. Each run therefore brings the tap to the highest published stable
`vMAJOR.MINOR.PATCH` release rather than to its own tag, and whichever run
goes ahead leaves the tap current; rerunning any stable release's tap job
repairs it. Swift app releases and prereleases never count. A tap that
already holds a newer formula or cask keeps both unchanged, so nothing
downgrades either. If another commit, such as the Swift release's, changes
`Formula/jet.rb` or `Casks/jet-app.rb` while the job runs, the job fails
without pushing; rerun it.

Failed size gates retain build artifacts for seven days but prevent publication.
Sizes and the profile each executable builds with are recorded in
[Core distribution](../../docs/core-distribution.md); this workflow does not relax
any budget. macOS GUI signing and notarization remain separate distribution steps.

`release-size.yml` runs the same package and gate steps for the three labels on
pull requests that change `packages/`, the packaging files, `release.py`, or
`setup-rust.sh`, and lists every executable's stripped size per architecture
slice in the job summary. It builds the profiles release.toml configures and
fails when a slice is over its budget.

`packaging.yml` rehearses the Linux x86_64 half of a release on pull requests
that touch packaging inputs, and on demand: it builds and gates the core
payload, bundles the desktop app unsigned, drives the unsigned `.deb` through
the desktop journey, and runs the Homebrew check. It uses no release secret
and is not a required check. The journey also runs when it, the timing
marks it reads, or the app's provisioning code changes. On every app change,
`just e2e-dry-run` in `apps/jet-tauri`, part of the app's `just check`, runs
it against fakes, and the app's tests run its page script against the real
components. Dispatch `packaging.yml` by hand before the first `v*` tag, so
the journey's first real run is not the one that gates `publish`. When only
the size gate fails, the later jobs still run on the uploaded payload so one
run reports every problem.

## Swift app releases

Changes under `apps/jet/` on `main` trigger `swift-release.yml` independently
of core version tags. The same workflow runs when its packaging scripts or
workflow change. Its run number sets the app version to `1.0.<run number>` and
creates a `swift-v1.0.<run number>` release in this repository. It builds a
universal macOS app and core payload from the same commit, bundles the payload
inside `jet.app`, and publishes `jet-app-<version>.dmg` plus the macOS core
archive. On first launch the app
stages and activates that payload and starts a user LaunchAgent. If a
Homebrew-managed daemon already owns the Plane, the app connects to it.

The release job uses Ape Bonker's existing GitHub App secret to publish
`Casks/jet.rb` in `apexgang/homebrew-tap`. The cask depends on the tap's
`apexgang/tap/jet` core formula. If the tap has no formula, or an older core
version, this job publishes a macOS-only formula from the same archive and
the same `jet.rb.in` template. The tagged core release replaces it with the
full macOS and Linux formula at the same core version, and this job leaves
that formula alone until a newer core version ships. The Linux desktop cask
`Casks/jet-app.rb` belongs to the tagged core release. Rerunning the job uses
the published checksums; an older run cannot downgrade either tap package.
The DMG is ad-hoc signed and not notarized yet.
macOS Gatekeeper may require approval to open it the first time. Once opened,
core installation needs no separate download or command.

This job builds with the core version on `main`. A run after the commit that
bumps the version in `packages/Cargo.toml` leaves the tap with a macOS-only
formula at the new version, and Linux installs and upgrades of
`apexgang/tap/jet` and `apexgang/tap/jet-app` fail until that version's tag
updates the tap. Push the `v*` tag at the version-bump commit and let its
release finish before merging changes under `apps/jet/`. If the release or
its tap job fails, rerun it; a patch release for an older version cannot
replace the newer formula. See
[Core distribution](../../docs/core-distribution.md) for the details.

## Validation

Run `python3 -m unittest discover -s .github/tests -v`, `actionlint`, and
`gh actions-lock --verify-local` after editing automation. The release tests
run `ruby -c` on the rendered formula and cask, so they need Ruby, and sign
and verify updater signatures with the `openssl` command (OpenSSL 3.0 or
later). Regenerate action pins in enrolled workflows with `gh actions-lock`.
Run `just release-envelope` and `just fmt` from `packages/` after moving
release tooling or changing its inputs.

`swift-release.yml` uses literal commit SHAs for every external action and is
not enrolled in `actions.lock`: the lockfile tool rewrites those refs to tags,
which would undo the required SHA pins. Verify its action SHAs against the
upstream release tags when updating them.
