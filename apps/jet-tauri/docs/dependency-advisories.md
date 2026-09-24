# Dependency advisories

Status: reviewed on 2026-09-24 for issue #205.

The Tauri app's CI job fails on every known vulnerability in its dependencies.
`just audit` runs `bun audit` over the whole `bun.lock` and
`cargo deny check advisories` over `src-tauri/Cargo.lock`. This page lists the
only advisories the audit accepts and explains why they are safe to merge.

An accepted advisory can still appear in Dependabot or in a raw `bun audit`, so
a merge can look as if it ships a known vulnerability. It does not: each one
below was checked against the three rules that follow, and CI rejects any other
advisory.

## When an advisory may be accepted

All three conditions must hold:

1. **No upgrade exists.** No release of the dependency we control permits the
   fixed version. The upgrade is impossible, not postponed.
2. **Jet cannot reach it.** Nothing in the dependency graph calls the affected
   code, or the code never ships in the app.
3. **The exception is narrow.** CI ignores exactly one advisory ID, so every
   other advisory, including a new one in the same package, still fails.

Record a new exception here, in the ignore list it needs (`src-tauri/deny.toml`
or the `audit` recipe in `justfile`), and in the pull request that adds it.

## Accepted advisories

### glib 0.18: `VariantStrIter` is unsound

- **IDs:** RUSTSEC-2024-0429, GHSA-wrw7-89jp-8q8g (medium, unsound).
- **Problem:** `glib::VariantStrIter`, returned by
  `glib::Variant::array_iter_str()`, passes an immutable reference as a C
  out-argument. Optimized builds can then dereference a null pointer and crash.
  The fix exists only in glib 0.20.
- **No upgrade:** every Tauri Linux stack requires `gtk ^0.18` and
  `webkit2gtk =2.0.2`, which require `glib ^0.18`. This covers `tauri` 2.11.6
  (the latest stable release, which Jet uses), `tauri-runtime-wry` 2.11.4,
  `wry` 0.57.0, `tao` 0.37.0 and `tauri` 3.0.0-alpha.2, as checked on crates.io.
  `gtk` 0.19 uses glib 0.22, but no WebKitGTK binding or Tauri release uses it.
- **Not reachable:** outside glib's own tests, no crate in the graph calls
  `array_iter_str` or `VariantStrIter`. The check covered every crate that
  depends on glib directly: atk, cairo-rs, gdk, gdk-pixbuf, gdkx11, gio, gtk,
  javascriptcore-rs, muda, pango, soup3, tao, tauri, tauri-runtime,
  tauri-runtime-wry, tauri-plugin-dialog, tauri-plugin-fs,
  tauri-plugin-notification and webkit2gtk. Jet's own Rust code does not use
  glib.
- **Enforced by:** the ignore entry in `src-tauri/deny.toml`. The Dependabot
  alert is dismissed as "vulnerable code is not used" with a link to this
  section.
- **Revisit:** when Tauri, wry or tao move to gtk 0.19 or to WebKitGTK 6
  bindings. Then upgrade and delete the exception.

### cookie 0.6.0 through SvelteKit

- **IDs:** GHSA-pxg6-pf52-xh8x, CVE-2024-47764 (low).
- **Problem:** `cookie` before 0.7.0 accepts out-of-bounds characters in a
  cookie's name, path and domain, so attacker-controlled values could change
  other fields of a `Set-Cookie` header.
- **No upgrade:** the latest `@sveltejs/kit`, 2.70.3, requires `cookie ^0.6.0`.
- **Not reachable:** SvelteKit uses `cookie` only in its server runtime. The app
  is a static single-page build (`adapter-static`) without server hooks or
  cookies, and its built output contains no cookie code. The package runs only
  during `vite build` on developer and CI machines, without untrusted input.
- **Enforced by:** `bun audit --ignore=GHSA-pxg6-pf52-xh8x` in the `audit`
  recipe.
- **Revisit:** when SvelteKit moves to `cookie` 0.7 or later.

## Unmaintained crates (informational)

`cargo deny` also marks six crates as unmaintained. Tauri pulls in all of them,
and none has an upgrade:

- `proc-macro-error` 1.0.4 (RUSTSEC-2024-0370), a build-time macro reached
  through `glib-macros` and `gtk3-macros`.
- `unic-char-property`, `unic-char-range`, `unic-common`, `unic-ucd-ident` and
  `unic-ucd-version` 0.9.0 (RUSTSEC-2025-0081, 0075, 0080, 0100 and 0098),
  reached through `urlpattern` and `tauri-utils`.

An unmaintained crate is not a vulnerability. `deny.toml` fails only when Jet
depends on an unmaintained crate directly.

## Fixed

- GHSA-82fw-gwwq-j7x9 (moderate): `vitest` and `@vitest/mocker` 3.2.7 allowed
  path traversal through a redirect mock. The app moved to vitest 5.0.1 (#205).

## Why the audit covers more than Dependabot

For this app Dependabot reads only `package.json`, so it misses transitive npm
packages such as `cookie`. `bun audit` reads the full `bun.lock`.
`cargo deny` reads the RustSec database directly, including unmaintained
notices that have no GitHub advisory. Run `just audit` in `apps/jet-tauri` to
reproduce CI. It needs `cargo-deny` and network access.
