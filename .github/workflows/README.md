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

1. Set the workspace version and push its matching `vMAJOR.MINOR.PATCH` tag.
2. The release workflow builds Linux x86_64, Linux ARM64, and universal macOS
   payloads, preserving the per-role profiles and enforcing the release envelope.
3. Once every gate passes, it uploads six archives, `SHA256SUMS`, and `jet.rb`
   into a draft release, then publishes the complete release.
4. For a stable release, Ape Bonker commits `Formula/jet.rb` to
   `apexgang/homebrew-tap`. `brew install apexgang/tap/jet` installs the compiled
   executables; `brew services start apexgang/tap/jet` starts the daemon.

The Actions secret `APE_BONKER_PRIVATE_KEY` holds Ape Bonker's private key.
`RELEASE_APP_CLIENT_ID` identifies the installed app; the workflow defaults to
the verified Ape Bonker client ID. `actions/create-github-app-token` restricts
the short-lived token to `contents:write` on `homebrew-tap` and revokes it at job
completion. The normal `GITHUB_TOKEN` publishes Jet releases. PR jobs receive
neither the private key nor the tap token.

Prerelease tags must also match the workspace version. They publish prerelease
assets without modifying the stable formula. Rerun failed jobs to retry a tap
update. A full rerun keeps published assets intact and downloads the original
formula, so rebuilt checksums cannot replace the published ones. Dispatch the
workflow against a version tag, never a branch. Older releases cannot downgrade
the formula after a newer stable release is published.

Failed size gates retain build artifacts for seven days but prevent publication.
Sizes and the profile each executable builds with are recorded in
[Core distribution](../../docs/core-distribution.md); this workflow does not relax
any budget. GUI signing and notarization remain separate distribution steps.

`release-size.yml` runs the same package and gate steps for the three labels on
pull requests that change `packages/`, the packaging files, `release.py`, or
`setup-rust.sh`, and lists every executable's stripped size per architecture
slice in the job summary. It builds the profiles release.toml configures and
fails when a slice is over its budget.

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
`Casks/jet.rb` in `apexgang/homebrew-tap`. The cask depends on the tap's `jet`
core formula. If the tap has no formula, or an older core version, this job
publishes a macOS formula from the same archive. The tagged core release may
later replace it with the full macOS and Linux formula. Rerunning the job uses
the published checksums; an older run cannot downgrade either tap package.
The DMG is ad-hoc signed and not notarized yet.
macOS Gatekeeper may require approval to open it the first time. Once opened,
core installation needs no separate download or command.

## Validation

Run `python3 -m unittest discover -s .github/tests -v`, `actionlint`, and
`gh actions-lock --verify-local` after editing automation. Regenerate action
pins in enrolled workflows with `gh actions-lock`. Run `just release-envelope` and `just fmt` from
`packages/` after moving release tooling or changing its inputs.

`swift-release.yml` uses literal commit SHAs for every external action and is
not enrolled in `actions.lock`: the lockfile tool rewrites those refs to tags,
which would undo the required SHA pins. Verify its action SHAs against the
upstream release tags when updating them.
