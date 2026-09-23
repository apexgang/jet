# Jet Tauri app

Cross-platform desktop client built with Tauri 2, SvelteKit, Svelte 5,
TypeScript, and Bun. Run frontend and Tauri commands from `apps/jet-tauri/`.

## Product boundaries

- `jetd` owns authoritative Jet state. This app owns presentation, desktop
  integration, and disposable client-local state.
- Use the versioned Jet protocol for every Plane. Send durable changes through
  protocol Commands and rebuild visible state from Queries, snapshots, and
  Events. Do not reproduce backend policy in TypeScript or the Tauri shell.
- Use the glossary in `../../CONTEXT.md` and the relevant ADRs under
  `../../docs/adr/`. Keep their domain terms in UI state, tests, and code.
- Read `../../PRODUCT.md` before making product or visual-design decisions. The
  starter screen is not an established Jet design language.

Use the Modrinth App as a reference for a focused desktop workspace, explicit
activity and progress states, typed events, and separation between the frontend,
Tauri shell, and core logic. In Jet, `jetd` already owns the core. Do not create a
second authoritative app backend or copy Modrinth's Vue structure, branding, or
controls.

## Structure

Keep the webview unprivileged and the Rust shell narrow.

- Keep `src/routes/` focused on route composition and page-level loading.
- Put reusable UI and browser-side logic under `src/lib/`, grouped by feature.
  Co-locate a feature's components, state, pure helpers, IPC adapter, and tests.
- Keep direct `invoke`, event, plugin, and window API calls in typed adapters
  under `src/lib/`. Components consume those adapters instead of command names.
- Keep browser-independent TypeScript pure so it can run without a Tauri
  webview.
- Keep `src-tauri/src/lib.rs` focused on builder setup, plugins, managed state,
  command registration, and lifecycle wiring. Move substantial commands into
  feature modules.
- Make each Tauri command validate and translate its input, delegate the work,
  and return a typed result. Core Jet behavior belongs in the backend workspace.

Before adding a package, Tauri plugin, global store, or generic component, check
whether the platform, Svelte, or existing code already solves the problem. Keep
new dependencies behind a focused adapter and document why they need native or
webview access.

## Svelte and TypeScript

Use strict TypeScript and current Svelte 5 runes syntax for new code.

- Use `$state` only for reactive state, `$derived` for computed state, and
  `$effect` for external side effects. Return cleanup functions from effects.
- Treat props as changing inputs. Represent mutually exclusive UI states with
  discriminated unions instead of Bool and optional-field combinations.
- Model loading, empty, stale, offline, partial, failed, and recovery states
  where the feature can reach them. Preserve the last trustworthy snapshot
  during reconnects when the protocol permits it.
- Give every Tauri event listener one lifecycle owner. Await registration and
  call its unlisten function when the component or feature scope ends.
- Guard asynchronous updates against stale completions when selection,
  navigation, or connection identity changes.
- Keep SvelteKit in static SPA mode. Add client routes, not server routes,
  server load functions, or runtime Node dependencies.
- Use semantic HTML and native controls first. Preserve keyboard-only use,
  visible focus, screen-reader names, reduced motion, high contrast, light and
  dark appearances, and usable layouts at the supported minimum window size.
- Open external URLs through the scoped Tauri opener adapter after validating
  the target. Keep navigation inside the webview for app routes.

Before using an unfamiliar Svelte API, consult the current
[Svelte best practices](https://svelte.dev/docs/svelte/best-practices). Treat
compiler accessibility warnings as defects unless the code records a narrow,
reviewed reason.

## IPC and desktop security

Treat the webview as an untrusted caller of privileged Rust APIs.

- Use commands for typed request/response work, events for lifecycle or state
  notifications, and channels for ordered or high-volume streams.
- Keep command arguments and results explicit and serializable. Return stable,
  structured error categories; UI behavior must not parse Rust, operating-system,
  network, or plugin error strings.
- Make filesystem, process, URL, deep-link, and network inputs pass narrow
  validation in Rust. Canonicalize paths before applying scope checks.
- Use async commands for I/O and long work. Keep synchronous commands short so
  they cannot block the webview main thread.
- Grant each window only the Tauri capabilities and plugin permissions it uses.
  Scope paths and URLs narrowly. Adding a permission requires a matching call
  site and a review of the generated capability schema.
- Treat custom commands as privileged APIs even when Tauri allows them to every
  bundled window by default. Restrict command exposure when a new window has a
  different trust level.
- Keep scripts and application assets bundled. Replace the template's `csp: null`
  with a restrictive Content Security Policy before loading remote content or
  producing a releasable build; extend only the directives a feature needs.
- Keep credentials, private keys, tokens, and sensitive payloads out of the
  webview bundle, browser storage, logs, fixtures, screenshots, and source
  control. Use the authoritative backend or an OS-backed secret store.

Before changing commands, capabilities, CSP, plugins, updater behavior, deep
links, or packaging, consult the current [Tauri 2 documentation](https://v2.tauri.app/)
for that branch and the generated schemas under `src-tauri/gen/schemas/`. Tauri 1
allowlist examples do not apply to this app.

## Wire contracts

`src/lib/protocol/JetModels.ts` is generated by `jet-protocol`. Never edit it by
hand.

Before changing wire DTOs, schemas, generated models, negotiation, or fixtures,
read `../../docs/wire-contracts.md` and `../../packages/AGENTS.md`. Regenerate
contracts from their Rust source with the documented `just` recipes.

- Validate incoming frames against the negotiated schema before using generated
  models.
- Preserve required raw JSON before parsing numbers, as the wire-contract
  documentation specifies.
- Keep protocol transport and negotiation outside Svelte components. Expose
  typed feature operations and state to the UI.

## Commands

Use Bun and keep `bun.lock` as the only JavaScript lockfile. Change it only when
dependencies change. Use `bun install --frozen-lockfile` for a reproducible local
install.

After frontend changes:

1. Run `bun run check`.
2. Run `bun run build`.

After Rust, Tauri configuration, capability, or plugin changes:

1. Run `cargo fmt --manifest-path src-tauri/Cargo.toml --check`.
2. Run `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`.
3. Run `cargo test --manifest-path src-tauri/Cargo.toml`.
4. Run `bun run tauri build --debug` when the change affects native wiring,
   permissions, bundled resources, or packaging.

Use `bun run tauri dev` for interactive checks and `bun run tauri info` when
diagnosing toolchain or platform failures.

## Tests

- Test pure TypeScript state and transformations without a webview.
- Test IPC adapters with `@tauri-apps/api/mocks`; clear mocks after each test.
- Test Rust service logic without Tauri where possible, then cover command
  translation and validation at the boundary.
- Prefer deterministic fakes for transport, clocks, filesystem, and native APIs.
  Keep real network access and wall-clock sleeps out of automated tests.
- Exercise critical desktop journeys in the running app on every affected
  operating system. A frontend-only test cannot prove native permissions,
  window behavior, menus, deep links, or packaging.

If a task introduces the first frontend test setup, prefer Vitest, add an
explicit `test` script to `package.json`, and keep the setup limited to the
behavior under test.

## Working style

- Inspect the relevant implementation and current official API documentation
  before editing. Verify Tauri permission names against generated schemas rather
  than recalling them from memory.
- Follow existing feature patterns unless the task explicitly changes them.
- Keep changes focused. Avoid unrelated refactors and speculative abstractions.
- Prefer compiler, schema, and test feedback over assumptions about IPC or
  platform behavior.
- When a task crosses the webview, Tauri shell, and backend, inspect every
  affected interface before changing any of them.

## Completion

Before finishing:

1. Review the diff for hand-edited generated code, lockfile drift, broad
   permissions, disabled security settings, and unrelated changes.
2. Run the narrowest relevant checks above, then build every affected target.
3. Exercise changed UI states in `tauri dev` at relevant window sizes,
   appearances, and input methods.
4. Confirm the worktree contains no secrets, build output, or temporary files.
5. Report the observable change, checks run, and any unverified operating system.
