# Wave 2.3: Tauri delivery and completion

The Deliver work-panel tab supports the existing Plane operations: create a
branch, commit a retained Turn checkpoint, push a named remote, and create or
update a GitHub draft PR with an explicit base branch. A native review holds an
immutable typed request. Confirmation sends only its opaque review ID. The
webview receives no generic Command, Git process, filesystem or shell bridge.

Acceptance is shown separately from completion. History reads the newest 100
`GitDeliveries` records and preserves each step's completed, pending, failed or
unknown outcome. An uncertain admission retries the same Command ID and body;
a definite daemon refusal permits a new review. Acknowledging an unknown Effect
has its own confirmation and never repeats Git. In-flight requests survive
navigation and reconnect within the running app. After relaunch, inspect the
Plane's delivery history; native review tokens are not persisted. At capacity, the oldest unsent
review expires. Submitted requests, including cached receipts that the webview
may not have received, are always retained. After 256 submitted requests, resolve
pending work and restart the app before making another delivery.

## Remaining protocol dependency

Protocol minor 35 does not expose a Git delivery preview containing the current
branch, HEAD and resolved remote URL, nor a confirmation binding for those
observations. The UI shows the selected remote name, explicit PR base, working
tree kind and checkpoint, and asks the user to verify the current branch and
remote in the working tree. It does not infer a current branch from historical
delivery results or read repository paths locally to simulate a Plane query.

A fully bound destination/branch preview requires a backend contract before
this part of the Wave 2.3 acceptance criteria can be considered complete. Swift
implementation and wire-schema changes are outside this change.

## Desktop notifications

Settings contains a master opt-in and independent approval, completion and
failure toggles. Preferences stay in native app data. `tauri-plugin-notification`
is used only in Rust because OS notification delivery is unavailable to the
unprivileged webview. No notification plugin IPC permission is granted.

Native validated events select fixed notification copy without task, path or
tool content. Initial snapshot and reconnect catch-up cursors suppress history;
a monotonic high-water mark deduplicates multiple feeds. Enabling notifications
fences against current Plane status. Disabling works offline. Notifications
require the app to be open and remain subject to OS notification settings.
Synchronous plugin errors are available when Settings is reopened. The desktop
plugin does not report asynchronous OS delivery failures or verify that a
notification was displayed.

## Verification

Run from `apps/jet-tauri` using the repository's Rust 1.98.1 toolchain:

```sh
bun install --frozen-lockfile
bun run check
bun run test
bun run build
cargo +1.98.1 fmt --manifest-path src-tauri/Cargo.toml --check
cargo +1.98.1 clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo +1.98.1 test --manifest-path src-tauri/Cargo.toml
RUSTUP_TOOLCHAIN=1.98.1 bun run tauri build --debug --bundles deb
```

The shared `just contracts-check` also passes without regenerated wire models.
Tests cover exact-request retries, navigation and stale asynchronous replies,
offline freshness, explicit refusals, acknowledgement without Git replay,
partial results, bounded inputs, safe PR URLs, event classification and native
notification filtering/deduplication.

Visual checks use the real Svelte components with mocked IPC for populated
history and confirmation, including light/dark and the 900×600 minimum viewport.
The scoped axe audit reports no violations. Linux native smoke checks cover
startup, Settings loading and preference persistence while the Plane is offline.
This Wayland host needs `WEBKIT_DISABLE_DMABUF_RENDERER=1` to run WebKit; that
local workaround is not embedded in application configuration.

The Linux debug executable and Debian bundle are the packaging checks for this
change. RPM and AppImage release packaging are not part of this verification.

A live Plane delivery to Git/GitHub and receipt of an OS notification from a real
Run have not been exercised. macOS is not verified by this Tauri-only change.
