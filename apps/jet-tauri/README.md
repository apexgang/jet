# Jet desktop for Linux

The Linux client uses Tauri 2, Svelte 5, and the shared Jet protocol. It is an
independent application: its native Rust shell talks to `jetd` through
`packages/jet-client`; it does not call or link the Swift application.

The cross-client architecture and frozen Wave 0 boundary are recorded in
[ADR-0106](../../docs/adr/0106-freeze-independent-desktop-client-foundations.md).

## Foundation boundary

- The webview has one read-only command, `open_plane_feed`.
- The command returns a bounded Plane status and streams redacted event labels.
- Socket paths, client identity, credentials, connection proofs, command bodies,
  and Event payloads remain native.
- The main window capability grants no opener, shell, filesystem, or broad core
  permission.

## Verify

Run these commands from this directory:

```sh
bun install --frozen-lockfile
bun run check
bun run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run tauri build --debug --bundles app
```
