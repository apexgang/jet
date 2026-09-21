# Jet Swift app

Native SwiftUI client for Jet. Treat macOS as the primary workspace and iOS as
the remote companion. Run commands from the repository root unless a command
changes directory explicitly.

## Product boundaries

- `jetd` owns authoritative Jet state. The app owns presentation, Apple-platform
  integration, and disposable client-local state.
- Use the same versioned Jet protocol for local and remote Planes. Do not add a
  direct Rust FFI path or a GUI-only mutation path.
- Send durable changes through protocol Commands and rebuild visible state from
  Queries, snapshots, and Events. Do not reproduce core policy in Swift.
- Use the glossary in `../../CONTEXT.md` and the relevant ADRs under
  `../../docs/adr/`; keep their domain terms in UI state, tests, and code.
- V1 product platforms are macOS and iOS. An Xcode project destination is not a
  product commitment to another platform.

Use the Modrinth App as a product reference for a focused desktop workspace,
clear activity states, and the separation of GUI from core logic. Reinterpret
those ideas with native Apple interaction and Jet's domain. Do not copy its
Vue/Tauri patterns, branding, or web controls.

## Structure

Keep the app target as composition and presentation around a narrow Jet client.

- Keep `jetApp` focused on scenes, dependency construction, and app lifecycle.
- Group substantial code by feature as the app grows. Keep a feature's views,
  state, actions, and focused helpers together.
- Keep SwiftUI views declarative. Move protocol I/O, coordination, and nontrivial
  state transitions into focused observable models or clients.
- Pass dependencies at scene or feature boundaries. Use environment values only
  for genuinely shared app context.
- Add a protocol when it marks a real boundary such as transport, credentials,
  clock, or filesystem behavior. Prefer a concrete type for a single internal
  implementation.
- Keep platform branches at narrow seams instead of scattering `#if os(...)`
  through feature logic.
- Prefer small value types and exhaustive enums for state. Avoid ambiguous Bool
  combinations and invalid optional combinations.

Before adding a package, check whether Apple frameworks or existing project code
already solve the problem. Record why a new dependency belongs in the app and
keep it behind a narrow boundary.

## Swift and concurrency

Follow the checked-in deployment targets, language mode, and actor-isolation
settings. Change them only when the task explicitly includes a migration.

- Use structured concurrency and propagate cancellation.
- Keep UI-owned state on `MainActor`. Keep blocking work and protocol I/O off the
  main actor.
- Give long-lived mutable resources one clear owner, usually an actor or an
  isolated observable model.
- Make values crossing isolation boundaries `Sendable`. Treat
  `@unchecked Sendable`, detached tasks, continuations, locks, and callback
  bridges as reviewed escape hatches with tests for their safety assumptions.
- Tie every long-running task to a scene, feature model, or explicit service
  lifetime, and cancel obsolete work when selection or navigation changes.
- Model loading, empty, stale, offline, partial, failed, and recovery states
  explicitly. Preserve the last trustworthy state during reconnects when the
  protocol permits it.

## SwiftUI and native interaction

Before changing navigation, windows, toolbars, menus, settings, permissions, or
platform-specific interaction, consult the current Apple Human Interface
Guidelines and relevant framework documentation.

- Start with system scenes, navigation, controls, typography, semantic colors,
  materials, and SF Symbols. Custom UI must communicate a Jet-specific object or
  interaction better than the system component.
- On macOS, account for resizing, multiple windows where useful, focus, keyboard
  navigation, menu commands, context menus, toolbar actions, and state
  restoration. Put the complete command model in menus; reserve toolbars for
  frequent contextual actions.
- On iOS, preserve purpose and terminology while adapting navigation, density,
  presentation, and touch targets. Do not shrink the Mac interface into a phone
  layout.
- Use Observation for new view state. Own an `@Observable` reference with
  `@State`, and use `@Bindable` only where a child needs editable projections.
  Keep constructors and `body` free of I/O and other side effects.
- Design each feature's first-run, empty, loading, disconnected, error,
  destructive, and recovery states with its happy path.
- Use localized user-facing strings. Layouts must tolerate longer text and
  right-to-left direction.
- Preserve Dynamic Type, VoiceOver, keyboard-only operation, increased contrast,
  reduced transparency, reduced motion, and light and dark appearances.
- Give nontrivial views previews with representative data and at least one edge
  state. Inject side effects so previews never require a live Plane.
- Add stable accessibility identifiers at UI-test boundaries. Keep visible and
  accessibility labels meaningful to people rather than to test code.

## Protocol, persistence, and security

`Protocol/JetModels.swift` is generated by the Rust protocol package. Never edit
it by hand.

Before changing wire DTOs, schemas, generated models, negotiation, or fixtures,
read `../../docs/wire-contracts.md` and the backend instructions in
`../../packages/AGENTS.md`. Regenerate contracts from their Rust source.

- Validate incoming frames against the negotiated schema before using generated
  models, and preserve required raw JSON as the wire-contract documentation
  specifies.
- Render stable Jet error categories and recovery actions. Never parse native,
  Rust, SSH, SQLite, or operating-system error strings for behavior.
- Use SwiftData or preferences only for client-owned local state or rebuildable
  caches. Their contents must not become authoritative Jet state.
- Store credentials and private keys through Keychain-backed platform code.
  Keep secrets out of SwiftData, preferences, logs, previews, fixtures,
  diagnostics, and source control.
- Log identifiers and payloads only when the diagnostic contract permits them;
  redact by default at transport and error boundaries.

## Tests

- Use Swift Testing for unit and integration tests. Keep XCTest for UI tests and
  APIs that require it.
- Test observable behavior and state transitions, including cancellation,
  reconnect, stale responses, malformed input, and recovery where relevant.
- Inject deterministic fakes for transport, clocks, credentials, and other true
  boundaries. Avoid real network access and wall-clock sleeps in tests.
- Prefer complete-value assertions and parameterized tests for meaningful input
  matrices.
- Add UI tests for critical cross-feature journeys, platform integration, or
  behavior that unit tests cannot establish. Launch into a controlled state.

## Commands

Discover schemes and destinations when Xcode configuration changes:

```sh
xcodebuild -list -project apps/jet/jet.xcodeproj
xcodebuild -showdestinations -project apps/jet/jet.xcodeproj -scheme jet
```

Use a temporary Derived Data directory so command-line checks leave the worktree
clean:

```sh
xcodebuild -project apps/jet/jet.xcodeproj -scheme jet \
  -destination 'platform=macOS' -derivedDataPath /tmp/jet-derived \
  CODE_SIGNING_ALLOWED=NO build

xcodebuild -project apps/jet/jet.xcodeproj -scheme jet \
  -destination 'platform=macOS' -derivedDataPath /tmp/jet-derived \
  CODE_SIGNING_ALLOWED=NO test -only-testing:jetTests
```

For shared UI or app code, also compile the iOS target:

```sh
xcodebuild -project apps/jet/jet.xcodeproj -scheme jet \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath /tmp/jet-derived-ios CODE_SIGNING_ALLOWED=NO build
```

Choose a concrete, launch-capable destination from `-showdestinations` for UI
tests and run the relevant `jetUITests` case explicitly. When wire contracts
change, run the contract recipes required by `../../docs/wire-contracts.md` from
`packages/`.

## Completion

Before finishing:

1. Review the diff for hand-edited generated code and unrelated changes.
2. Run the narrowest relevant tests, then build every affected product platform.
3. Exercise changed UI in previews or the running app across its relevant states,
   window sizes, appearances, and input methods.
4. Confirm the worktree contains no Derived Data, credentials, or temporary
   artifacts.
5. Report the observable change, checks run, and any unverified platform state.
