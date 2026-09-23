# Jet desktop implementation plan

Status: Wave 0, Wave 1, and Waves 2.1–2.2 were completed on 2026-09-22. The Swift app completed Wave 2.3 on 2026-09-23; Tauri Wave 2.3 remains unimplemented. Generic Harness approval decisions remain a backend dependency.

This plan turns `docs/design-language.md` into a staged desktop product for macOS and Linux. It is repository-specific and preserves the boundaries in `apps/jet/AGENTS.md`, `apps/jet-tauri/AGENTS.md`, and the Jet protocol ADRs.

## Outcome

Ship a trustworthy desktop client in which a casual user can:

1. Connect Jet, choose a Project, and start a task without learning Jet's internal architecture.
2. Follow a durable Conversation across Runs and reconnects.
3. Approve, queue, interrupt, stop, and recover work with clear consequences.
4. Inspect changes, files, terminals, delivery, schedules, and system health when needed.
5. Use the same mental model on macOS and Linux.

macOS is the interaction reference and lands first within each vertical slice. Linux follows from the same fixture set and acceptance criteria. The clients share protocol contracts and presentation fixtures, not a UI framework.

## Delivery rules

- Build vertical slices rather than completing one entire client before the other.
- Keep `jetd` authoritative. Clients own only transient presentation state, drafts, selection, window state, and explicitly client-local preferences.
- Never edit generated protocol models by hand. Contract changes begin in the protocol source, regenerate both clients, and update conformance fixtures.
- Put all core access behind one typed adapter per client so views cannot issue ad hoc commands.
- Do not advance a slice until its empty, loading, offline, stale, denied, unsupported, failure, and recovery states are testable.

## Target architecture

| Concern | macOS | Linux | Authority |
| --- | --- | --- | --- |
| UI | SwiftUI scenes, split view, inspector, menus, Settings | Svelte 5 routes and components inside Tauri 2 | Client presentation only |
| Session state | One `@MainActor @Observable` desktop session model per scene | One root session controller with feature-scoped Svelte state | Derived from Plane snapshots and events |
| Protocol access | Native Swift transport and typed client actor | Rust shell reusing `packages/jet-client` | `jetd` |
| Streaming | Async sequences over a framed connection | Tauri channels from the Rust shell | Plane cursor order |
| Secrets | Keychain | OS-backed secret service through Rust | Local operating system |
| Persistence | Scene restoration and non-sensitive client preferences | Tauri window state and non-sensitive preferences | Never browser storage for Jet content |

### Planned macOS modules

Keep the first decomposition shallow and feature-oriented:

```text
apps/jet/jet/
  App/
  Client/
  DesignSystem/
  Features/
    Shell/
    Sidebar/
    Conversation/
    Composer/
    WorkPanel/
    Setup/
    Settings/
  PreviewSupport/
  Protocol/JetModels.swift
```

`Client/` owns framing, validation, authentication, queries, commands, event resumption, and stable error mapping. Views receive presentation models and intents, not wire requests.

### Planned Linux modules

Keep protocol authority in Rust and the webview narrow:

```text
apps/jet-tauri/
  src/lib/
    jet/
    features/
    ui/
  src-tauri/src/
    jet/
      client.rs
      commands.rs
      channels.rs
      errors.rs
  src/lib/protocol/JetModels.ts
```

The Rust shell connects through `packages/jet-client`. Typed request and response commands handle bounded operations. Tauri channels carry ordered, high-volume streams. Global events are limited to genuinely global, low-volume shell notifications.

## Wave 0: de-risk the foundations

Goal: prove the architecture before building the shell.

### 0.1 Contract and fixture inventory

Status: completed on 2026-09-22.

- Map each confirmed screen and action to its query, command, response, event, capability, stable error, and revision rule.
- Create a checked-in presentation fixture corpus for first launch, ready, active, queued, approval, completed, offline, stale cursor, denied, unsupported, and recovery states.
- Record unsupported UI actions rather than mocking them with local state.
- Add a protocol-generation freshness check to both app verification paths.

Exit: every Wave 1 and Wave 2 interaction has a named protocol path or a tracked backend dependency.

### 0.2 Swift transport spike

Status: completed on 2026-09-22.

- Prove a local UNIX-domain connection, framed JSON request and response, authentication handshake, event stream, cancellation, reconnect, and cursor resume.
- Evaluate the structured-concurrency `NetworkConnection` API against `NWEndpoint.unix(path:)` on the current deployment target.
- If that combination is unsuitable, wrap `NWConnection` in one actor and expose async sequences. Do not leak callbacks into feature code.
- Validate wire envelopes before typed decoding and map server errors to stable presentation errors.

Exit: a non-UI integration test can connect to a real or hermetic `jetd`, run one query and command, consume ordered events, disconnect, and resume.

### 0.3 Tauri bridge spike and shell hardening

Status: completed on 2026-09-22.

- Add `packages/jet-client` to the Rust shell and prove the same query, command, stream, reconnect, and cursor-resume path.
- Replace the starter `greet` surface with a minimal typed bridge boundary.
- Replace `csp: null` with a restrictive policy that includes at least `object-src 'none'` and `base-uri 'none'`, then add only measured exceptions.
- Remove unused opener access and split capabilities by window and feature. Treat all webview arguments and rendered content as untrusted.

Exit: the webview can render a sanitized fixture and a live connection status without receiving credentials, private keys, connection proofs, or unrestricted filesystem and shell authority.

### 0.4 Architecture decision checkpoint

Status: completed on 2026-09-22 in [ADR-0106](adr/0106-freeze-independent-desktop-client-foundations.md).

Capture the spike results in an ADR or an amendment to this plan. Freeze the adapter interfaces, connection state machine, error taxonomy, fixture format, and client-state ownership before shell implementation.

The checkpoint freezes both apps as independent peer clients of `jetd`, with shared protocol and presentation contracts but no shared runtime bridge. Wave 1 changes to a frozen boundary require a superseding ADR.

Estimated effort: 2 to 4 engineer-weeks. The Swift production transport and decoder are the largest uncertainty.

## Wave 1: first useful task

Goal: a new user can connect locally, choose a Project, start work, watch it, and return to it.

### 1.1 Native shell

Status: completed on 2026-09-22 for the independent SwiftUI and Tauri clients.

- Replace both starter screens with the sidebar, Conversation column, composer, and contextual work-panel shell.
- Implement macOS menus, shortcuts, focus routing, window restoration, and Settings scene using native SwiftUI APIs.
- Add the Jet accent token `#29B6F6`, semantic colors, typography, spacing, selection, and accessibility states.
- Build components against fixtures before connecting them to live adapters.

### 1.2 Local setup and Projects

Status: completed on 2026-09-22 for the independent SwiftUI and Tauri clients.

- Detect and connect to the local Plane, show service health, and preserve meaningful stable errors.
- List, register, select, and remove Projects using protocol commands and capability checks.
- Connect a Harness or provider account through a client-safe flow; keep credentials outside the presentation layer.
- Make remote pairing visible but skippable.

### 1.3 Conversation vertical slice

Status: completed on 2026-09-22 for the independent SwiftUI and Tauri clients.

- List and search Conversations with pagination and stable selection.
- Create a Conversation, create or start a Run, submit a Turn, and stream useful activity into the timeline.
- Group low-value raw events and preserve the protocol's order within each Plane.
- Support relaunch, offline cache labeling, reconnect, cursor resume, and full snapshot recovery after cursor expiry.

Verification: rebuilt SwiftUI and Tauri application bundles were visually inspected in their offline and reconnecting states. Automated tests cover the protocol and client behavior in this slice. This validation pass did not complete a task against a live local daemon.

Exit: on macOS and Linux, a new user can complete one local task, relaunch the app, and return to the same Conversation. VoiceOver or keyboard-only operation covers every critical action on macOS.

Estimated effort: 3 to 5 engineer-weeks after Wave 0.

## Wave 2: controlled work and inspection

Goal: users can safely supervise real work and inspect its result.

### 2.1 Turns, approvals, and run control

Implemented on 2026-09-22 in both desktop clients. The clients render the bounded authoritative queue, allow ownership-checked withdrawal, show observed Run revisions, preserve command IDs for uncertain retries, present structured approval requests and automatic-review retry, and keep Interrupt Turn separate from Stop Run through confirmation and terminal feedback. The current protocol has no generic Harness approval-decision command, so the clients explicitly disclose that limitation and offer only supported cancellation controls; they do not invent Approve or Reject effects.

- Render the full turn queue with position, target, withdrawal, and the protocol's maximum-size behavior.
- Present approvals inline with action, target, scope, consequence, and clear reject or cancel actions.
- Implement Interrupt Turn and Stop Run as distinct commands with distinct confirmation and lifecycle feedback.
- Preserve command IDs and expected revisions across retries. Never infer a successful effect from a disconnected response.

### 2.2 Work panel

Implemented on 2026-09-22 in both desktop clients. Changes page incrementally, retained patch Artifacts load in bounded verified chunks, Files stay within native-issued Project or Workspace bindings, and Workspace terminals use multiplexed byte-credit streams without exposing arbitrary paths or shell construction to the Tauri webview. Current, final, Turn, and historical checkpoints are selectable. Structured recovery, restart metadata, and revision-conflict safe state drive named recovery actions. Refresh preserves loaded pages, file identity, drafts, terminal transcript continuity, and panel selection; split UTF-8, terminal control sequences, and insertions before a selected later-page file have regression coverage.

Verification: the Swift macOS unit suite and iOS Simulator build passed; the Tauri Svelte/TypeScript checks, frontend tests and build, strict Rust lint, and native tests passed; the shared `jet-client` formatting, strict lint, and tests passed. Both macOS bundles were visually reviewed in the truthful reconnect/no-Run state because no live local Plane was available for populated capture.

- Build Changes with incremental file lists, bounded diff loading, binary and oversized-file states, and review actions.
- Add scoped Files and Terminals without exposing arbitrary host paths or shell commands through the webview.
- Add Run details, grouped activity, artifacts, checkpoints, and recovery actions.
- Preserve selection and scroll position across streaming updates and panel collapse.

### 2.3 Delivery and completion

Swift app status: implemented on 2026-09-23. The native client gates delivery on the observed Git capability, requires review of the operation, destination, branch, and retained checkpoint before admission, and renders the durable outbox's pending, completed, confirmed-failure, and unknown-outcome states. Confirmed failures can be reviewed for a new attempt; unknown outcomes can only be acknowledged after review and are never presented as safe automatic retries. Approval, completion, and failure notifications are individually opt-in, device-local, deduplicated, and use generic lock-screen-safe content. The Tauri client remains out of scope for this implementation and is not complete.

- Implement supported commit, push, and GitHub pull-request delivery paths with explicit destination and branch review.
- Show partial success and retry states from the transactional outbox rather than presenting optimistic completion.
- Add opt-in desktop notifications for approvals, completion, and failure with in-app controls.

Exit: a user can supervise a non-trivial Run, review hundreds of changed files without loading them all, control execution safely, and deliver supported work.

Estimated effort: 4 to 7 engineer-weeks after Wave 1.

## Wave 3: complete desktop product

Goal: cover the confirmed onboarding, pairing, settings, and recovery scope.

### 3.1 Planes and pairing

- Pair, confirm, limit, disable, and revoke remote clients with explicit actor and Plane identity.
- Connect remote Planes only through the encrypted SSH standard-I/O path. Never fall back to plaintext.
- Aggregate sidebar and Search results client-side while preserving each Plane's independent cursor and failure state.
- Present unsupported capabilities per Plane without hiding the rest of the product.

### 3.2 Settings and automation

- Implement the five confirmed Settings groups with capability-aware controls and stable deep links from errors.
- Add schedules, accounts, usage, Harness extensions, execution defaults, reviews, retention, and notification routing.
- Keep policy enforcement in `jetd`; settings UI edits versioned values and reports conflicts.

### 3.3 Recovery, retention, and system health

- Implement Jet Trash, forget, restore, delete everywhere, auto-delete review, recovery snapshots, and disk-pressure states.
- Add diagnostics and structured audit views with sensitive fields redacted by default.
- Surface background service versions, capabilities, degraded states, and safe repair actions without turning the main workspace into a metrics dashboard.

### 3.4 Parity and adaptation

- Close the semantic parity matrix between macOS and Linux.
- Test narrow, default, wide, full-screen, multiple-display, dark, increased-contrast, reduced-motion, and reduced-transparency configurations.
- Keep iOS compile-safe and defer remote-companion product work to its own approved plan.

Exit: every section of `docs/design-language.md` has implemented acceptance coverage on both desktop platforms or an explicitly approved platform exception.

Estimated effort: 8 to 14 engineer-weeks after Wave 2, with backend gap work included only where noted below.

## Wave 4: release hardening

Goal: prove safety, performance, accessibility, recovery, and distribution.

- Run the full contract, unit, integration, UI, accessibility, security, reconnect, crash-recovery, and soak suites on release artifacts.
- Measure launch, idle CPU, memory, initial Conversation render, large diff navigation, long stream behavior, and reconnect against `docs/resource-budgets.md` and `docs/release-contract.md`.
- Test schema skew, unsupported capabilities, duplicate commands, revision conflicts, partial Plane outages, disk pressure, credential revocation, and corrupted local presentation state.
- Complete signing, packaging, update, rollback, privacy, dependency, and supply-chain gates for both clients.

Exit: the release contract is green, no high-severity accessibility or security findings remain, and recovery drills preserve authoritative Jet state.

Estimated effort: 3 to 5 engineer-weeks after feature completion.

## Verification strategy

### Shared contract layer

Use the same versioned fixture corpus to verify Swift decoding, Rust `jet-client`, the Tauri adapter, and both presentation layers. Include unknown additive fields, malformed envelopes, out-of-order Plane data, stable errors, large payloads, and capability differences.

### macOS gates

For each slice, require Swift unit tests for reducers or presentation models, client integration tests, SwiftUI component or snapshot coverage where stable, UI tests for the critical path, and manual VoiceOver plus full-keyboard checks. Run the target's configured `xcodebuild` test and build commands from `apps/jet/AGENTS.md`.

### Linux gates

For each slice, require Rust unit and integration tests for commands, channels, capabilities, sanitization, and error mapping; Svelte component tests for state and keyboard behavior; and end-to-end tests for the critical path. Add explicit package scripts for check, test, and build before feature work relies on them.

### Cross-client acceptance

Record a compact parity matrix by user-visible capability, not by component. A capability passes only when wording, consequence disclosure, state recovery, protocol semantics, and accessibility are equivalent, allowing native platform differences.

## Security acceptance

The desktop implementation must satisfy the following ASVS-aligned controls at its boundaries:

1. Validate and safely decode every untrusted envelope and apply contextual output encoding, including agent text, diffs, file names, URLs, and errors (ASVS 1.2.1 through 1.2.5, 1.5.2, 2.1.1 through 2.2.3).
2. Preserve trusted sequencing, limits, atomicity, command deduplication, expected revisions, authorization, and Actor identity instead of reproducing them in the client (ASVS 2.3.1 through 2.3.4, 8.1.1, 8.1.2, 8.3.1 through 8.3.3).
3. Harden the webview with a documented restrictive CSP, narrow Tauri capabilities, safe external navigation, and no client-side security boundary (ASVS 3.1.1, 3.4.3 through 3.4.6, 3.7.2, 3.7.3).
4. Keep paths, terminals, files, uploads, and remote connections scoped to registered roots and encrypted transports (ASVS 5.1.1, 5.3.2, 12.3.1).
5. Keep secrets and sensitive Jet content out of logs and browser storage, minimize retention, and use structured redacted errors and audit events (ASVS 13.3.1, 13.3.2, 14.1.1, 14.1.2, 14.2.3 through 14.3.3, 16.1.1 through 16.5.2).

Dependency review, lockfile integrity, generated-code provenance, and update isolation are release gates (ASVS 15.2.1 through 15.2.5). Shared state and streaming code require race, cancellation, and reentrancy tests (ASVS 15.4.1 through 15.4.4).

## Known protocol and product dependencies

These are not reasons to block shell and first-task work. They must be resolved before their dependent features ship.

| Dependency | Current evidence | Plan |
| --- | --- | --- |
| Conversation layout and pins | ADR 0034 defines shared and private layouts, but current generated client commands do not expose layout or pin operations | Add or locate the authoritative protocol surface before implementing Pinned. Do not fake shared pins with local storage |
| Plane transfer GUI | `docs/plane-transfer.md` states that GUI bundle transport and a transfer query remain separate work | Treat transfer UI as backend-dependent; keep recovery and normal multi-Plane use independent |
| Swift production codec and validation | Generated Swift declarations exist, but the app has no production transport, codec, or schema-validation layer | Resolve in Wave 0.2 and gate all Swift feature work on the proven adapter |
| Tauri authority boundary | The starter shell has `csp: null`, opener permission, and only a greeting command | Resolve in Wave 0.3 before rendering live Jet content |
| Cross-Plane aggregation | Events are ordered within a Plane, not across Planes | Maintain one cursor and health state per Plane; merge only presentation results with visible provenance |
| Backend release blockers | `docs/release-contract.md` tracks daemon size, startup, parity, macOS failures, and performance evidence | Track separately from GUI development, then require closure for desktop release |

## Risk controls

| Risk | Early signal | Control |
| --- | --- | --- |
| Swift client work expands unpredictably | UNIX transport, framing, validation, or cancellation fails in the spike | Time-box Wave 0.2 and freeze the client interface only after a real round trip |
| Two clients drift | Fixtures or wording fork during the first slice | One parity matrix, shared fixtures, and Linux completion inside every slice |
| Streaming overwhelms UI | Timeline updates cause scroll jumps, high CPU, or memory growth | Batch presentation updates, group activity, bound caches, and add long-stream soak tests |
| Client weakens authority | UI invents success, retries unsafe effects, or stores shared state locally | Central adapters, command IDs, revisions, stable errors, and no optimistic durable state |
| Scope hides critical paths | Settings and rare recovery flows consume the schedule before task execution works | Protect the Wave 1 first-task exit and sequence advanced surfaces after it |

## Proposed implementation slices

Once implementation is separately authorized, use this order for reviewable pull requests:

1. Foundation checks, fixture corpus, and protocol UI matrix.
2. Swift transport spike and typed client boundary.
3. Tauri `jet-client` bridge, CSP, capabilities, and typed frontend adapter.
4. Shared visual states and macOS shell, followed by Linux shell parity.
5. Local setup and the create, run, stream, reconnect Conversation slice.

Later pull requests should follow the Wave 2 through Wave 4 boundaries rather than combine unrelated settings, recovery, and work-panel features.

## Schedule range

The current range is 20 to 35 engineer-weeks for one experienced engineer, excluding substantial new backend protocol implementation. Two engineers split by platform or transport and product surfaces could target roughly 12 to 20 elapsed weeks, but the Swift transport spike and unresolved protocol surfaces make that range uncertain.

No implementation should begin until the first slice has an owner, a branch strategy, and explicit authorization to change code.
