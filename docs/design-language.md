# Jet desktop design language

Status: confirmed for implementation planning on 2026-09-21.

This document is the design authority for Jet's main desktop experience. It covers the native macOS app in `apps/jet/` and the Linux desktop app in `apps/jet-tauri/`. The current ChatGPT desktop application is the interaction reference for both clients. Jet does not copy ChatGPT branding, assets, product modes, or visual identity.

## Product intent

Jet is a calm desktop workspace for directing coding agents without requiring users to understand process supervision, terminals, or Jet's internal architecture.

The casual user wins product tradeoffs. A first-time user should be able to describe work, understand where it will run, approve consequential actions, follow progress, and inspect the result. Expert controls remain available through contextual detail, menus, keyboard shortcuts, and the work panel.

The primary object is a durable Conversation. A Conversation outlives individual Runs, app restarts, Plane connections, and Harness choices. The interface should feel like returning to ongoing work, not reopening a process monitor.

## Product principles

1. Start with the task. Let users type before requiring configuration.
2. Make authority visible. Show where work runs and what it can affect before a consequential action.
3. Reveal operational detail when it becomes useful. Keep raw events, terminals, revisions, and protocol language out of the default reading path.
4. Preserve continuity. Reconnection, relaunch, recovery, and a new Run should not make a Conversation feel disposable.
5. Prefer native, familiar behavior. Use platform controls, menus, focus, selection, restoration, and accessibility semantics.

## Vocabulary

Jet's domain terms remain precise in diagnostics, settings, advanced controls, and documentation. Everyday interface copy uses concrete values first.

| Domain concept | Default presentation | Detail presentation |
| --- | --- | --- |
| Conversation | Task or the task's name | Conversation |
| Plane | This Mac or the device name | Plane name, connection, and capability details |
| Harness | Codex, Claude Code, or another product name | Harness identifier, version, account, and extensions |
| Run | Activity within the task | Run name, lifecycle, execution mode, and revision |
| Turn | A user message or queued request | Turn status, queue position, and control actions |
| Craft | Capability or extension name | Craft package, permission, lifecycle, and host access |
| Visa | Supervised run | Visa policy and execution details |
| No Visa | Direct run | No Visa policy and execution details |

Use the established distinction between **Interrupt Turn** and **Stop Run**. Never collapse them into an ambiguous Stop action.

## App and window model

Version 1 uses one restorable main workspace window and a separate native Settings scene.

- The main window supports resizing, full screen, multiple displays, sidebar collapse, and work-panel collapse.
- Closing the main window does not imply stopping Runs or disconnecting Planes.
- Reopening Jet restores the last useful Conversation unless a needs-attention item should take precedence.
- Multi-window Conversations are deferred until restoration, selection ownership, and command routing are proven in the single-window model.
- The iOS target remains buildable, but its remote-companion experience is outside this desktop plan.

Initial implementation assumptions, delegated to the implementation team, are a 1280 by 800 point default window and a 900 by 600 point minimum. The work panel adapts to an overlay or closes below approximately 1100 points. These values may move during visual QA without changing the design language.

## Main workspace

The workspace uses a three-region structure.

### Sidebar

The sidebar is persistent at comfortable widths and collapsible through the toolbar, View menu, and keyboard shortcut. Keep its hierarchy broad and no deeper than two visible levels.

Order the primary destinations as follows:

1. New task, Search, and Needs attention.
2. Projects.
3. Pinned and Recent tasks.
4. Schedules.
5. Planes and Settings as lower-frequency destinations.

Do not place critical health or approval information only at the bottom. Needs-attention items appear near the top and use concise badges rather than a dashboard of metrics.

### Conversation

The center column is the reading and composing surface.

- The header identifies the task, Project, current execution status, and where active work runs.
- The timeline shows user turns, useful agent output, approvals, results, and recovery events.
- Raw activity is grouped into expandable summaries. It should not overwhelm the task narrative.
- The composer remains available while a Run is active when the protocol allows queued turns.
- A compact context row below the composer shows Project, Harness, and **Runs on**. Missing required context is resolved inline when the user sends.

#### Selection, continuity, and activity

- Search and paginated lists preserve stable Conversation identity and selection, including when the selected Conversation is outside the current page or search result set.
- Starting the first task is one create-and-start operation: create the Conversation, start its Run, and include the first Turn atomically. Later messages use the Turn submission path and retain a stable command ID across exact-body retries.
- The selected Conversation determines the Project shown in its header and context. New-task Project choices must not overwrite the context of an existing Conversation.
- Timeline activity is ordered by its Plane cursor and fenced to the active Conversation. Project only the known, safe fields of recognized events into the narrative; group unknown or low-value raw activity without interpreting or executing it.
- Reconnect from the last accepted cursor when possible. If the cursor has expired, discard the potentially gapped projection and rebuild it from a full snapshot before resuming the stream.
- Cached presentation remains available while offline when useful, but it is visibly labeled stale. Live surfaces must show unavailable or missing data truthfully and must never substitute fixture, preview, or synthetic values.
- Persist only the selected Conversation UUID as client-local restoration state. Keep Conversation content in bounded memory caches, and never place prompts, outputs, event payloads, or other Jet content in browser storage.

### Work panel

The trailing work panel is contextual, resizable where the platform supports it, and hidden by default when it has no useful content.

Its primary tabs are Changes, Files, Terminal, and Queue or Run details. It is an inspector, not a second navigation system. A selected diff, file, approval, artifact, or terminal should preserve the Conversation as the surrounding context.

On narrow windows, present the panel as an overlay or focused destination. Never compress the conversation into an unreadable strip.

## Core flows

### First launch

First launch has three essential outcomes: connect to the local Jet service, connect an account or Harness, and choose or add a Project. Pairing a remote Plane is visible but optional and skippable.

Use progressive onboarding inside the real workspace. Avoid a long introductory carousel. Provide safe defaults, explain why access is needed at the moment it is requested, and allow the user to revisit setup from Settings.

### Setup and Projects

Keep setup inside the main workspace. Present local Plane health, Projects, Harness access, and remote pairing as independent sections so one unavailable source does not replace usable setup data.

- Keep connection status independent from setup data. If capabilities, Projects, accounts, or pairing fails, leave successful sections usable and show the stable error code in the affected section. Do not label a connected Plane offline because one section could not load.
- Add a Project through the platform's folder picker, then show the resolved working-tree root and whether Jet can register it. On Tauri, manual absolute-path entry is a disclosed expert fallback.
- Before removing a Project, show an authoritative preview of its folder, disk use, changed files, unpushed commits, managed Workspaces, and any blockers. Do not enable removal without a current preview. When blockers exist, explain them and keep confirmation unavailable.
- Move to Trash is the default removal action. Offer permanent deletion only when Trash is unavailable and after showing the permanent-deletion warning.
- Project removal uses a modal confirmation that blocks interaction outside the dialog. Require the exact Project name, focus the confirmation field or Cancel when removal is blocked, keep focus within the dialog, support Escape to cancel, and return focus to the invoking control.
- Harness setup records only a non-secret binding. Credential entry and sign-in remain with the Harness or an operating-system credential flow; credentials never pass through the presentation layer.

### New task

Opening New task focuses an empty composer immediately. The user can type before choosing a Project, Harness, or Plane.

When the user sends, Jet validates the required context. If one item is missing, keep the draft intact and present the smallest inline choice that resolves it. If multiple valid execution paths exist, recommend the safest familiar default and show the chosen **Runs on** value before starting.

### Active run

Stream meaningful progress into the timeline. Group repetitive tool and protocol activity behind a concise summary with an activity count and latest state.

Approvals appear inline where the action was proposed. Each approval must state the action, target, scope, consequence, and whether it can be remembered. Reject and cancel remain easy to reach.

Interrupt Turn stops the active response while preserving the Run. Stop Run ends the Run. Queue actions show their target and queue position and use the protocol's withdrawal semantics.

### Completion and delivery

On completion, emphasize the outcome and changed work. The work panel opens Changes when a useful diff is available. Delivery actions use plain labels such as Commit, Push, or Create pull request and disclose destination, branch, and irreversible effects before execution.

### Return and recovery

Relaunching opens the last Conversation or the most urgent recoverable item. Reconnection keeps cached presentation clearly marked as stale, resumes from the last cursor when possible, and explains when a full refresh was required.

Recovery, Jet Trash, retention, and auto-delete use concrete dates and consequences. Destructive actions identify which Plane or Planes are affected and whether recovery remains possible.

## States and scale

Every primary feature needs designed states for loading, empty, ready, stale, offline, permission denied, unsupported capability, partial failure, and recovery.

The interface must remain usable with:

- Thousands of Conversations across Projects and Planes.
- Hundreds of changed files in one Run.
- Long streaming Runs with substantial grouped activity.
- Up to 128 queued turns where the protocol permits them.
- Multiple terminals, schedules, accounts, and remote Plane connections.

Use pagination, incremental rendering, stable identity, bounded caches, and virtualization where measurements justify it. Never load the full history or a complete large diff only to render an initial screen.

## Visual language

Jet is near-monochrome, flat, and native. The visual system uses platform materials, separators, system typography, and standard selection behavior.

- Brand accent: `#29B6F6`. In alpha-first 32-bit contexts use `0xFF29B6F6`.
- Use the accent for selection, primary actions, focus reinforcement, links, and branded status accents.
- Reserve red, orange, yellow, and green for semantic danger, warning, pending, and success states. Do not recolor semantic states with the brand accent.
- Meet platform contrast requirements in light, dark, increased-contrast, and reduced-transparency modes.
- Avoid gradients, heavy shadows, decorative glass, excessive cards, and ornamental motion.

Spacing, type scale, corner treatment, and controls follow platform defaults before introducing custom tokens. Custom components must have a concrete interaction need and preserve native focus, keyboard, VoiceOver, and high-contrast behavior.

## Motion and feedback

Motion explains continuity, selection, disclosure, and panel changes. It does not decorate streaming work.

- Respect Reduce Motion and Reduce Transparency.
- Do not animate every incoming event.
- Use determinate progress only when Jet knows the total.
- Preserve stable scroll position when activity is appended above a visible composer or approval.
- Notifications are optional, truthful, and reserved for approvals, completion, failure, or another meaningful state change.

## Menus, commands, and input

Every toolbar command also appears in the menu bar on macOS. Menus expose the complete command set and keep unavailable commands visible but disabled when that aids discovery.

The planned command model includes:

- File: New task, add or open Project, close window.
- Edit: standard editing commands, local find, global Search.
- Conversation: send, interrupt turn, stop run, rename, fork, archive or forget when allowed.
- View: show or hide sidebar, show or hide work panel, choose work-panel tab.
- Window and Help: standard platform behavior and Jet help or diagnostics entry points.

Keyboard shortcuts must not shadow standard text-editing commands. Pointer, trackpad, keyboard, VoiceOver, Switch Control, and full keyboard access are first-class input paths.

## Settings

Settings is opened from the application menu and `Command-,` on macOS. It restores the last pane and uses a pane-style toolbar when the platform provides one.

Group settings into five areas:

1. General: appearance, notifications, launch, restoration, and local behavior.
2. Agents: Harnesses, accounts, models where supported, and extensions.
3. Work: defaults for Projects, workspaces, reviews, delivery, schedules, retention, and auto-delete.
4. Connections: local service, remote Planes, pairing, SSH, and paired clients.
5. Safety and system: execution policy, permissions, storage pressure, recovery, diagnostics, audit, and versions.

Do not put preferences in the main workspace toolbar. A contextual action may link to the exact Settings pane that resolves an issue.

## Security and privacy behavior

The GUI presents authority but does not recreate core policy. `jetd` remains authoritative for authentication, authorization, revisions, command deduplication, sequencing, and durable state.

- Store local secrets in Keychain on Apple platforms and an OS-backed secret service on Linux. Never store prompts, outputs, credentials, connection proofs, or private keys in browser storage.
- Preserve command IDs, expected revisions, Actor identity, and Plane identity across every adapter boundary.
- Treat the Tauri webview as untrusted. Expose narrow typed commands and channels, validate generated wire messages before use, and enforce a restrictive content security policy.
- Open external URLs only through explicit allowlists and show the destination when the transition could surprise the user.
- Render agent and tool output as untrusted content. Do not execute embedded markup, scripts, file paths, or commands.

Errors use stable Jet error codes when available, a user-readable explanation, and a safe recovery action. Diagnostic detail is opt-in and must exclude secrets and sensitive content by default.

## Platform adaptation

macOS is the primary interaction and visual reference. Use SwiftUI's native scene, split-view, inspector, Settings, menu, focus, and restoration APIs.

Linux follows the same information architecture, wording, state model, and Jet brand, while using familiar desktop conventions available through Tauri and the system webview. Pixel identity is not a goal. Semantic and behavioral parity is.

Both clients consume the same versioned Jet protocol and conformance fixtures. They do not share a rendering layer.

## Evidence ledger

The following sources informed this record. They were accessed on 2026-09-21 unless noted otherwise.

| Source | Evidence used | Design consequence |
| --- | --- | --- |
| [Apple HIG design principles](https://developer.apple.com/design/human-interface-guidelines/design-principles) | Purpose, agency, responsibility, familiarity, flexibility, simplicity, craft, and delight | Casual-user clarity, visible consequences, native behavior, and careful detail are release criteria |
| [Designing for macOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos/) | Resizable windows, menu completeness, keyboard access, personalization, multiple displays, long sessions | Restorable resizable window, complete menus, shortcuts, and no mobile-style fixed canvas |
| [Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars) | Broad hierarchy, hide and show behavior, restrained depth | One collapsible sidebar with no more than two visible levels |
| [Toolbars](https://developer.apple.com/design/human-interface-guidelines/toolbars) | Frequent contextual actions and menu equivalents | Small contextual toolbar; full command set remains in menus |
| [Searching](https://developer.apple.com/design/human-interface-guidelines/searching) | Clear primary search with optional scopes | One global Search destination with optional Project or Plane filters |
| [Settings](https://developer.apple.com/design/human-interface-guidelines/settings) | App-menu access, `Command-,`, pane navigation, last-pane restoration | Separate Settings scene with five stable groups |
| [Onboarding](https://developer.apple.com/design/human-interface-guidelines/onboarding) | Optional, fast, contextual teaching and reasonable defaults | Progressive setup in the workspace; remote pairing is skippable |
| [Alerts](https://developer.apple.com/design/human-interface-guidelines/alerts) | Specific neutral copy and easy cancellation | Consequence-first approvals and destructive confirmations |
| [Privacy](https://developer.apple.com/design/human-interface-guidelines/privacy) | System credential storage and least disclosure | Keychain or secret-service storage and contextual permission copy |
| [Focus and selection](https://developer.apple.com/design/human-interface-guidelines/focus-and-selection/) | System focus and selection behavior | Native focus rings, selection, and full keyboard access |
| [Managing notifications](https://developer.apple.com/design/human-interface-guidelines/managing-notifications) | Consent, truthful urgency, and user control | Opt-in notifications only for meaningful state changes |
| [NavigationSplitView](https://developer.apple.com/documentation/swiftui/navigationsplitview) and [Inspector](https://developer.apple.com/documentation/swiftui/view/inspector(ispresented:content:)) | Native adaptive navigation and trailing inspection | SwiftUI root uses a split view with an adaptive inspector |
| [ChatGPT desktop overview](https://learn.chatgpt.com/docs/app) and [Projects](https://learn.chatgpt.com/docs/projects) | Conversation-first shell, persistent navigation, Projects, search, composer, and contextual work | Interaction grammar reference only; Jet retains its own domain and identity |

## Confirmed decisions and open dependencies

Confirmed product decisions are the casual-user priority, full desktop scope, ChatGPT desktop interaction reference, macOS-first implementation, Linux parity, and `#29B6F6` brand accent.

Implementation can adjust measurements and component decomposition through visual QA. It cannot change the information architecture, authority model, vocabulary distinctions, or core flows without updating this record and obtaining product confirmation.

Protocol dependencies that require resolution before their interface can ship are tracked in `docs/desktop-implementation-plan.md`.
