# Desktop design

## Brief and authority

Jet helps casual users describe technical work, follow its progress, and return
to it later. Experienced users can inspect and control execution. The user
requested a complete replacement of the existing visual design on September
26, 2026, explicitly prioritizing casual users and desktop behavior on Linux.
Presentation decisions are delegated. This record describes that replacement,
not an endorsement of the previous interface.

The existing typed clients, authoritative daemon, revision checks, pairing,
credential boundaries, and durable Command identities remain the integration
foundation. Views must never manufacture successful work.

## Work model

A window is a library of Conversations with one focused Conversation. The
primary loop is choose a Project and Harness, describe an outcome, follow the
Conversation, then inspect and keep the changes. A Project is a registered Git
repository. Each new Conversation uses its own Workspace at HEAD with no copied
local changes. Explain that as a separate working copy next to the selector.
Do not imply that an arbitrary folder is already a supported Project.

The library contains New task, Search, and recent Conversations. Projects,
Schedules, Planes, Trash, and Settings are secondary destinations. A Plane is
introduced as a computer running Jet. Retain domain terms in technical detail
and destructive reviews, without requiring people to learn them to start.

New work opens a centered, modest writing area with real Project and Harness
choices. Examples insert editable text; they never send it. Existing work uses
a full-height transcript and a bottom composer. Show byte limits only near
the bound. Show queued-input feedback when work is already underway.

The work panel is closed by default. Changes, Files, Terminal, Run, and Delivery
open through explicit commands. Narrow windows use the existing modal inspector
behavior with focus return. Closing a window or panel does not stop work.

Setup presents the two user choices, Project and Harness, with a clear next
action. Local service installation remains automatic. A healthy service is a
small status, while a failure exposes repair. Remote pairing is optional and
does not block local work.

## Visual system

Use warm off-white and charcoal surfaces, with copper reserved for primary
actions and selection. Semantic green, amber, and red retain their meanings.
macOS uses system surfaces, native selection, SF Symbols, menus, and controls.
Linux uses its native decorated window, semantic HTML controls in the Tauri
webview, native folder dialogs, and a compact application toolbar. No browser
navigation, page chrome, marketing sections, or imitation Mac traffic lights.

System sans serif typography, readable body text, and monospaced code. The
scale is 11 for metadata, 12 for controls, 13 for navigation, 14 for content,
17 for section titles, and 24 for the new-work invitation. Spacing follows
4, 8, 12, 16, 24, and 32. Controls use small radii; only the composer and
modal boundaries have larger radii. Sections use spacing or separators, not
nested cards. The transcript has a readable maximum width of 760 points.

No ambient animation or decorative blur. State transitions are short and
respect reduced motion. Focus stays visibly distinct from selection. Status
always has text and never depends on color alone.

## Interaction and state

Command/Ctrl+N starts new work, Command/Ctrl+K finds saved work,
Command/Ctrl+Return sends, and the existing platform shortcuts expose the
sidebar, inspector tools, settings, window close, and quit. Return remains a
newline in the composer. Native dialogs own focus and Escape; overlays return
focus to their invoker. Stop Run and Interrupt Turn remain distinct reviewed
actions. Project removal retains exact path/risk review and typed confirmation.

Connection failure preserves the last trustworthy content and user input.
Commands are unavailable until the selected Plane is connected. Uncertain
Commands retain their identity and exact body. No automatic success or retry
is inferred from a missing acknowledgement. A changed Harness choice applies
only to the next new Run and is explicit before sending.

Presentation text remains inert. Svelte interpolates text rather than injecting
HTML, applying ASVS 1.2.1. Inputs are bounded and checked against known choices;
trusted native and daemon validation remains authoritative, ASVS 2.2.1 and
2.2.2. Error behavior uses structured categories, ASVS 16.5.3.

## Platform evidence

Sources consulted September 26, 2026. No deployment target changes or beta-only
APIs are required.

| Choice | Guidance | Jet consequence |
| --- | --- | --- |
| Workspace with a library | [Designing for macOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos) and [Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars) | Stable selection, restrained navigation, content takes priority |
| Contextual tools | [Toolbars](https://developer.apple.com/design/human-interface-guidelines/toolbars) and [Panels](https://developer.apple.com/design/human-interface-guidelines/panels) | Frequent actions in the toolbar, details on demand |
| Short setup | [Onboarding](https://developer.apple.com/design/human-interface-guidelines/onboarding) | Explain choices where they occur, remote setup stays optional |
| Keyboard access | [Keyboards](https://developer.apple.com/design/human-interface-guidelines/keyboards) and [Focus and selection](https://developer.apple.com/design/human-interface-guidelines/focus-and-selection) | Native commands, focus restoration, visible keyboard focus |

Relevant components are buttons, menus, text fields, lists, split views,
inspectors, sheets, alerts, progress indicators, search, pickers, and settings.
Notifications are opt-in. File import uses native selection. Charts, maps,
media playback, purchases, widgets, cloud document sync, and device continuity
have no role in this desktop workflow. Dragging arbitrary files into a prompt
is deferred until a supported attachment contract exists.

## Verification and capability audit

### Implemented destinations

The audit used `CommandRequest`, `QueryRequest`, the clients, and the feature
documents below. Existing protocol behavior and confirmation steps were kept
while replacing the workspace, setup, navigation, typography, surfaces, and
control hierarchy. A visible task is a durable Conversation, not a Run.

| Capability | macOS SwiftUI | Tauri desktop | Contract |
| --- | --- | --- | --- |
| Register and inspect a Project; reviewed removal | Projects, native folder picker and removal sheet | Projects, native folder picker and removal dialog | [Project removal](project-removal.md), `RegisterProject`, `PreviewProject` |
| Choose installed Harness and start isolated work | New task, Project/Harness selectors | New task, Project/Harness selectors | [Managed Runs](managed-runs.md), `CreateConversation`, `StartRun` |
| Resume work, read history, search, rename | Task library and Conversation menu | Task library, Find command and task actions | `Conversation`, `Search`, `SetConversationName` |
| Submit and withdraw queued messages | Composer and Run details | Composer and Run details | [Turn queue](turn-queue.md) |
| Interrupt a turn, stop a Run, resolve orphaned execution | Reviewed controls and Run details | Reviewed controls and Run details | [Execution control](execution-control.md) |
| Inspect approvals and request an allowed review retry | Transcript approval details | Transcript approval details | [Automatic review](automatic-review.md), `AuthorizeApprovalRetry` |
| Inspect checkpoints, changed files, artifacts and diffs; edit files and send review comments | Changes and Files in work details | Changes and Files in work details | [Checkpoints](change-checkpoints.md), [edits and reviews](user-edits-and-reviews.md), [artifacts](artifacts.md) |
| Open, attach, detach and close Workspace terminals | Terminal in work details | Terminal in work details | [Workspace terminals](workspace-terminals.md) |
| Review branch, commit, push and GitHub draft PR operations; reconcile uncertain delivery | Delivery menu and panel | Delivery panel and native View menu | [Git delivery](git-delivery.md) |
| List, create and cancel daily schedules | Schedules shortcut opens Work Settings | Schedules destination with task selector and confirmation | [Schedules](schedules.md), existing protocol minor 21 |
| Scoped defaults, Auto-continue and automatic review settings | Work and Safety Settings | Work and Safety Settings | [Auto-continue](auto-continue.md), `Settings`, `SetSetting`, `ClearSetting` |
| Account bindings, usage, Craft installation/disable and native extensions | Harnesses Settings | Harnesses Settings | [Craft lifecycle](craft-lifecycle.md), [installation](craft-installation.md), [extensions](harness-extensions.md), [usage](usage-records.md) |
| Pair, inspect, disconnect and revoke access to remote computers | Connections | Connections and native View menu | [Remote connections](remote-connections.md) |
| Move to Jet Trash, delete everywhere with review, restore, manage grace period and Autodelete rules | Work Settings | Trash and Work Settings | [Autodelete](autodelete.md) |
| Storage/recovery state, audit, diagnostics, service and version information | Safety & System | Safety and system; Connections | [Recovery](recovery.md), [diagnostics](diagnostics.md), [disk pressure](disk-pressure.md) |
| Notifications and presentation restoration | Native Settings and scene state | Separate native Settings window and app-local presentation state | Existing opt-in notification and privacy contract |

Task naming uses revision-bound Commands, retains uncertain request identities,
and offers a new attempt against an explicitly accepted current version after
a conflict. Schedule creation and cancellation use the daemon's existing wire
contract through a narrow `jet-client` adapter. No schedule timer, policy,
credentials, or authoritative state moved into the webview.

### Deliberate exclusions and current limits

The main workflow uses a fresh managed Workspace at the Project's HEAD.
Choosing another Git starting point, copying uncommitted work, operating
directly in the Local checkout, and creating a no-Project Conversation are
excluded from this interface. These choices need their own working-tree and
data-loss review; they must not be inferred from a prompt or hidden preference.

The following advanced workflows remain protocol capabilities with no GUI
entry in this redesign. This is an explicit exclusion, not a claim that a
command-line product or a hidden button implements them:

- [Fork](conversation-forks.md) and [Handoff](handoffs.md) creation, external
  Harness history import/resume, and manual Run naming. Task naming and
  continuation cover the everyday library workflow.
- Workspace promotion into a permanent checkout and [Plane transfer](plane-transfer.md).
  They require destination mapping and authority review. Git delivery remains
  the implemented way to keep and share changes.
- Explicit [Visa](visa-runs.md) account selection, [No-Visa](no-visa-execution.md)
  origin/target setup, and No-Visa remote tool review. Paired task observation,
  continuation, and ordinary managed execution retain existing support.
  Swift can choose a computer for new managed work; Tauri's New task currently
  creates local work. This platform difference remains a product limitation.
- A standalone Utility request console, arbitrary artifact publication,
  portable Recovery bundle import/export, and raw lifecycle transitions.
  The GUI exposes purpose-specific delivery, review, recovery and settings
  operations instead. Internal state transitions are never editable controls.
- Prompt attachments and file dragging. The current input contract accepts
  text, not attachment references. No decorative drop zone is shown.

The general approval-request protocol does not expose an interactive
Approve/Reject Command. The transcript therefore reports the request and
available retry actions without manufacturing a decision. No-Visa tool review
is a separate protocol operation and is excluded above.

### Verification evidence

Checks performed on September 26, 2026 on macOS:

- Svelte type/accessibility checks and static production build.
- Full frontend test suite, including keyboard routing, inspector focus return,
  offline retention, theme contrast, inert message rendering, Command identity
  retention, schedule target changes and rename conflict recovery.
- Rust formatting, all-target Clippy with warnings denied, and the full Tauri
  Rust suite. The schedule wire round trip verifies create/query/cancel through
  an authenticated fake Plane and preserves decimal protocol values.
- Native Tauri `.app` bundle built and launched with bundled assets at
  `tauri://localhost`. New task, Find, Settings, the task starter and Schedules
  empty state were exercised in the real window. Find focused search; New task
  focused the composer; Settings opened a separate window; starter text stayed
  editable and was not sent. The final workspace spacing was visually checked.
- Swift desktop tests, including the real framed-protocol task-name exchange,
  and macOS/iOS Simulator compilation. The rebuilt native macOS workspace and
  Settings window were opened and inspected with accessibility and screenshots.
- `jet-client` formatting, Clippy and tests after adding schedule request
  methods. No wire models, lockfiles, runtime dependencies, credential storage,
  CSP, or network permissions changed.

These checks do not establish release readiness on Linux. This host has no
Linux desktop runtime, and neither a live authenticated provider run nor the
Linux package/install/update journey was exercised. Native window minimums and
compact inspector behavior have automated coverage; minimum-size resizing and
light appearance still need a desktop visual pass. The macOS debug Tauri bundle
has no bundled `jetd`, so its native runtime checks covered disconnected/setup
and local UI interactions. Connected mutations were verified by protocol and
state tests. These are open validation gates, not simulated successful runs.
