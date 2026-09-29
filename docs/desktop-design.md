# Desktop design

This record covers Jet's two desktop apps: the native macOS app in `apps/jet`
and the cross-platform Tauri app in `apps/jet-tauri`, which is the desktop app
on Linux. The shared foundations apply to both. The macOS sections describe the
confirmed redesign from issue #220. The Tauri sections record the September 26,
2026 replacement, which the macOS redesign does not change.

## Status and scope

| Part | Status | Dates |
|---|---|---|
| macOS app, `apps/jet` | Confirmed redesign target. Implementation verification pending. | Requested and confirmed September 28, 2026 |
| Tauri app, `apps/jet-tauri` | Implemented replacement, unchanged by the macOS redesign | Recorded September 26, 2026 |
| Apple guidance | Human Interface Guidelines and "Adopting Liquid Glass" | Accessed September 28, 2026 |

The macOS redesign has these limits:

- It targets macOS 26.5. Every API this record names is available by macOS
  26.1.
- The iOS target keeps compiling, with its fixture path intact. iOS gets no
  redesign.
- It builds on the colleague's branch
  `220-improve-the-design-of-desktop-applications-of-jet` at 1f0635b and is
  proposed as a pull request into that branch.
- It changes no backend code. Rust, the Jet protocol and `JetModels.swift`
  stay as they are. Where the design needs data the protocol lacks, it uses a
  bounded client fallback listed under [Protocol gaps](#protocol-gaps).
- Persisted keys keep their raw values, and changes to them are additive only.
  Accessibility identifiers stay.

The user asked to keep the visual gamut of the previous macOS design. It stays:

- the copper `AccentColor`, #A0472B in light mode. The dark fill moves from
  #E3A080 to #C15634, and #E3A080 stays as dark copper text (ledger C4);
- system surfaces;
- semantic orange, green and red, always paired with text;
- flat styling with radii 5 and 8;
- content widths of 760 points for reading and 640 points for writing;
- the type scale 11, 12, 13, 14, 17 and 24;
- the JetMark, shown only on New Task.

These safety invariants hold on every screen:

- Views send typed Commands only and never manufacture success.
- There is no Approve button, because no approval Command exists.
- Interrupt turn and Stop Run stay distinct, and each is confirmed.
- An uncertain Command keeps its identity and exact body and is never retried
  automatically.
- Project removal keeps its path and risk review and its typed-name
  confirmation.
- Forgetting a Conversation and deleting it everywhere stay confirmed.
- Renaming stays bound to the Conversation revision.

Implementation starts with behavior fixes. A refresh never takes over New
Task, each destination keeps its own draft, the person's own operations are
separated from background refresh, one send blocker feeds every Send control,
and the interface shows the real computer name.

## Brief and authority

Jet helps casual users describe technical work, follow its progress, and return
to it later. Experienced users can inspect and control execution. On September
26, 2026 the user requested a complete replacement of the existing visual
design, explicitly prioritizing casual users and desktop behavior on Linux. On
September 28, 2026 the user asked for a UI and UX redesign of the macOS app on
top of that work. It keeps the visual gamut and treats casual users as
first-class users, so the interface must be easy and intuitive for them.
Presentation decisions are delegated, and the ledger records the assumptions
made on the user's behalf.

The existing typed clients, authoritative daemon, revision checks, pairing,
credential boundaries, and durable Command identities remain the integration
foundation. Views must never manufacture successful work.

## Product understanding

On the Mac, Jet is the calm place to hand a coding task to an assistant, follow
it, stop it and keep the result. The assistant works in a separate copy of the
project. Nothing reaches the project until the person chooses.

The defining emotion is calm confidence: "I know what it's doing, I know what
needs me, and my project is safe."

A casual user is a developer who is new to Jet. Git words are fine. Jet words
never need to be learned: Plane, Harness, Craft, Run, Turn, Workspace, Visa,
Effect, checkpoint, revision, cursor and outbox appear only in Technical
Details, tooltips and destructive reviews. Experienced users reach the same
depth through the Details inspector and the menu bar.

Each moment of the main loop has one obvious next step:

1. A new task: Start Task.
2. A reply in progress or finished: Reply, or Interrupt.
3. Changes to keep: Keep Changes….

The macOS redesign does not cover approving or denying permission requests
(no Command exists), Workspace promotion, forks, Handoffs, prompt attachments,
multiple windows, an iOS redesign, or the app icon.

## Scenarios

Every macOS scenario can also be driven from the menu bar and the keyboard
alone.

1. First success. New Task shows an inline checklist. The person adds a
   project by confirming "Add “web-app”?". Start Task (⌘↩) opens the task once
   it exists.
2. Routine work. The sidebar shows spinners, copper new-reply dots and a Needs
   You section. The person replies, or presses ⌘. to interrupt. A "Reply
   Ready" notification opens the right task.
3. The next day. The task shows cached or replayed history, or a summary card
   when history is not available. Replying continues with the same assistant.
4. Keeping changes. Keep Changes… opens from the toolbar, ⇧⌘K, the transcript
   or Details. The person confirms one plan and sees "Saved to branch
   jet/fix-login-redirect" with Copy and a `git switch` line.
5. Expert work. Details (⌥⌘0) holds diffs, file editing, line comments,
   terminals and Technical Details. The Changes menu holds single Git steps.
6. Failure and recovery.
   - Offline: the content and the draft stay, with Try Again.
   - Permission request: Interrupt and Reply.
   - Unconfirmed Git step: the task moves to Needs You, with Mark as Checked….
   - Helper failure: the reason, with Try Again.

The design assumes people who work from the keyboard, use VoiceOver, or read
transcript text at up to 200%.

## Interface language

A task (Conversation) belongs to a project (a registered Git repository) on a
computer (Plane). Each task has a working copy (Workspace) at HEAD. Runs are
not named in the interface; only their status shows. A task is made of
messages and replies. Replies record changes, which the person keeps through
Git steps. Removed tasks go to Jet Trash. Removed project folders go to the
macOS Trash.

| Domain term | macOS interface word | Domain term may still appear in |
|---|---|---|
| Conversation · Turn (user / assistant) · Turn queue · withdraw | task · message / reply · Waiting to send · Remove | Technical Details |
| Run | not named | Technical Details, Stop confirmation |
| Interrupt turn · Stop Run | Interrupt · Stop Assistant… | Stop confirmation ("Stop Run") |
| Harness / Craft | assistant, or its product name | Settings › Advanced |
| Plane · Pairing · Paired client | computer, "This Mac" · Connect Another Computer · connected Jet app | Computers › Connection Details |
| Workspace · Local checkout | working copy · your project folder | Technical Details, destructive reviews |
| Change checkpoint | All Changes / Last Reply / Reply N | Technical Details |
| Git delivery · Outcome unknown · acknowledge | Keep Changes · Couldn't confirm · Mark as Checked | none |
| Approval request · Automatic review · retry grant | Needs permission · automatic safety review · Ask Reviewer Again | the permission card's Details |
| Account binding · Scheduled task | sign-in · Repeats daily | none |
| Forget · delete everywhere · Project removal | Move to Jet Trash · Delete Everywhere · Move Project Folder to Trash | their reviews |
| Autodelete rule · Recovery mode · work panel | Clean up idle tasks · "Jet paused changes to protect your data" · Details | Settings › Advanced |
| Revision, cursor, Effect, outbox, Visa, Artifact | never shown | Technical Details |

`CONTEXT.md` defines task and reply. A lint test checks casual interface
strings against the `CONTEXT.md` Avoid lists. The Tauri app keeps its current
words (see ledger O3).

## Design principles applied

- Purpose and simplicity. Each moment has one primary action, the app uses
  one status vocabulary, and expert detail sits behind disclosure. This
  resolves the tension between casual and expert users without two modes.
- Agency. Interrupt is always visible while a reply runs. Removing a queued
  message is reversible, and Move to Jet Trash can be undone. Confirmations
  appear only for irreversible or outward actions.
- Responsibility. Outward Git steps are reviewed. Jet states what leaves the
  Mac before the person chooses, and automatic push has its own confirmation.
- Familiarity. The window is a Mail and Notes style library with native
  selection, a title menu, a full menu bar, and ⌘. for Interrupt, which the
  HIG assigns to cancelling an operation.
- Flexibility. The whole loop works from the keyboard, with VoiceOver, and at
  200% text size. The inspector is never modal.
- Craft and delight. Liquid Glass comes only from system components. The
  defining moments stay calm.

## Shared foundations

Both apps follow these rules.

A window is a library of Conversations with one focused Conversation. The
primary loop is choose a Project and Harness, describe an outcome, follow the
Conversation, then inspect and keep the changes. A Project is a registered Git
repository. Each new Conversation uses its own Workspace at HEAD with no copied
local changes. Explain that as a separate working copy next to the selector.
Do not imply that an arbitrary folder is already a supported Project. Closing a
window or panel does not stop work.

Copper is reserved for primary actions and selection. Semantic green, amber,
and red retain their meanings. Status always has text and never depends on
color alone. Typography is the system sans serif with monospaced code. The
scale is 11 for metadata, 12 for controls, 13 for navigation, 14 for content,
17 for section titles, and 24 for the new-work invitation. Spacing follows 4,
8, 12, 16, 24, and 32. Controls use small radii; only the composer and modal
boundaries have larger radii. Sections use spacing or separators, not nested
cards. The transcript has a readable maximum width of 760 points. There is no
ambient animation or decorative blur. State transitions are short and respect
reduced motion. Focus stays visibly distinct from selection.

Stop Run and Interrupt Turn remain distinct reviewed actions. Project removal
retains exact path and risk review and typed confirmation. Connection failure
preserves the last trustworthy content and user input. Commands are
unavailable until the selected Plane is connected. Uncertain Commands retain
their identity and exact body. No automatic success or retry is inferred from
a missing acknowledgement. A changed Harness choice applies only to the next
new Run and is explicit before sending.

Presentation text remains inert. Inputs are bounded and checked against known
choices; trusted native and daemon validation remains authoritative, ASVS
2.2.1 and 2.2.2. Error behavior uses structured categories, ASVS 16.5.3.

## Tauri desktop app

This section is the September 26, 2026 record for `apps/jet-tauri`. The macOS
redesign leaves it unchanged.

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
behavior with focus return.

Setup presents the two user choices, Project and Harness, with a clear next
action. Local service installation remains automatic. A healthy service is a
small status, while a failure exposes repair. Remote pairing is optional and
does not block local work.

Surfaces are warm off-white and charcoal. The app uses its native decorated
window, semantic HTML controls in the Tauri webview, native folder dialogs, and
a compact application toolbar. There is no browser navigation, page chrome,
marketing sections, or imitation Mac traffic lights.

Command/Ctrl+N starts new work, Command/Ctrl+K finds saved work,
Command/Ctrl+Return sends, and the existing platform shortcuts expose the
sidebar, inspector tools, settings, window close, and quit. Return remains a
newline in the composer. Native dialogs own focus and Escape; overlays return
focus to their invoker.

Svelte interpolates text rather than injecting HTML, applying ASVS 1.2.1.

## Mac experience

### Window model

The macOS app is a library app, like Mail and Notes.

- The main scene is `Window("Jet", id: "main")`, 1280×800 by default and
  900×600 at minimum. Settings is a `Settings` scene.
- Open in New Window is deferred, because the session holds a single
  selection.
- `NavigationSplitView(columnVisibility:)` holds the sidebar and the detail.
  Details is an `.inspector` at every width.

| Column | Width in points (minimum / ideal / maximum) |
|---|---|
| Sidebar | 220 / 260 / 320 |
| Detail | at least 460 |
| Details inspector | 300 / 360 / 640 |

There are no custom bar backgrounds. `LegibleBarBackground` leaves the live
path.

```
+-------------------------+---------------------------------------------+-------------------------+
| (o)(o)(o) [sb] [new]    | Fix login redirect v   (spin) Working · Editing files |[Keep Changes…][stop][...][det]
|                         | web-app · Claude Code                       |                         |
+-------------------------+---------------------------------------------+-------------------------+
| [Search Tasks        ]  | You  Fix the redirect loop after login…     | Changes|Terminal|Activity|
| New Task                | Claude Code                                 | All Changes|Last Reply v |
| Needs You               |  > Used 3 tools · Read, Edit, Bash          | 3 files changed  +42 −7 |
|  (!) Update deps        |  The loop came from `redirectTo`…           | (+) src/auth/guard.ts   |
| Tasks                   |  (v) Claude Code changed files.             | (~) src/auth/login.ts   |
|  (spin) Fix login…      |      [Review Changes] [Keep Changes…]       |     @@ -10,6 +10,9 @@   |
|  ( • ) Add dark mode    | You  Also add a test.  Waiting to send · 1st |-------------------------|
| Projects             +  | +-----------------------------------------+ | Saved to branch jet/…   |
|  [folder] web-app       | | Send a follow-up…   [Interrupt][Send ↑] | |       [Keep Changes…]   |
| Jet Trash               | +-----------------------------------------+ |                         |
+-------------------------+---------------------------------------------+-------------------------+
```

Below a window width of 1100 points, opening Details collapses the sidebar to
`.detailOnly`. Closing Details, or widening the window, restores the previous
sidebar visibility. Details is never a modal sheet and never closes on resize.
Its content keeps the `work-panel` identifier. While the sidebar is
auto-collapsed, the wrapper also carries `compact-work-panel`.

### Selection, drafts and restoration

`enum SidebarItem { newTask, task(UUID), project(UUID), trash }` drives
`List(selection:)` and maps to `sidebarSelection`, `selectedConversationID`
and `selectedProjectID`. `SidebarDestination` gains `trash`. On restore, the
old `search`, `needsAttention`, `schedules` and `planes` destinations map to
`conversation`. The `needsAttention` fixture branch is unchanged.

Refreshes never replace New Task or the open task. Selection falls back only
when the task disappears. Jet then opens New Task and says "That task is no
longer available."

At launch, Jet restores `jet.shell.selection`. It reselects
`jet.last-conversation` only when that selection is a task and "Reopen the
last task" is on. It never redirects to Projects.

Each destination has its own draft, kept in memory only. A failed start keeps
the text. Unsaved file edits prompt "Save changes to login.ts?" with Save,
Don't Save and Cancel.

Persisted state:

- Kept: the three `jet.shell.*` SceneStorage keys, with `work-panel` now
  defaulting to `changes`, and all AppStorage keys.
- New keys: `jet.shell.sidebar-visibility`, `jet.transcript.text-scale`
  (0.85 to 2.0), `jet.transcript.show-technical`,
  `jet.sidebar.projects-expanded`, `jet.notifications.offer-shown` and
  `jet.settings.computer`.
- `ClientMemory` in UserDefaults: `jet.seen.v1`, `jet.task-assistants.v1` and
  `jet.trash-titles.v1`. It stores IDs, sequences and Jet Trash titles only,
  never prompts.
- Untouched: `jet.client-id` and `jet.remote-planes.v1`.

### Sidebar

The sidebar is `List(selection: $session.sidebarItem).listStyle(.sidebar)`,
which gives native highlight, arrow keys and type-select.
`.searchable(placement: .sidebar, prompt: "Search Tasks")` adds search. The
brand row, the ⌘N and ⌘K pseudo-buttons, the caption links,
`PlaneStatusFooter` and the custom row background are removed.

Rows appear in this order:

1. New Task (`square.and.pencil`). Its secondary line reads "Draft" while the
   draft has text.
2. Needs You, shown only when it is not empty.
3. Tasks, newest first by creation date, ending with "Show Earlier Tasks".
4. Projects, a `Section(isExpanded:)` of folder rows whose header has a `plus`
   button labelled "Add Project…".
5. Jet Trash (`trash`).

Each task appears in exactly one section. The sidebar toolbar holds New Task.

A task row has a 16-point status glyph, a one-line title with the full title in
`.help`, and a secondary line such as "web-app · 2 hr ago". In Needs You, the
status replaces the time: "web-app · Needs permission". "· Studio Mac" is added
only when there are two or more computers. Rows on an offline computer use the
secondary style and read "Offline". VoiceOver reads a row as one element:
"Fix login redirect, web-app, Needs permission, new reply".

| Sidebar state | Presentation |
|---|---|
| Loading | 4 redacted rows |
| Failed with nothing cached | "Couldn't load tasks" with Try Again |
| Live and empty | "Tasks you start appear here." |
| A remote computer offline | a top row "Studio Mac is offline" with Try Again |

Search filters titles instantly. 300 ms after typing stops, Jet searches every
computer. Hits go to "Other Matches", each showing the task title, or "Task on
Studio Mac", plus "Name:", "File:" or "Branch:". The index covers nothing else.
A spinner in the header shows progress, and a failure reads "Couldn't search
Studio Mac" with Try Again. No hits shows `ContentUnavailableView.search`.
Results stay until Esc clears the text. ⌘K reveals the sidebar and focuses the
field.

Return on a row focuses the composer. Delete opens Move to Jet Trash….
Arrowing through rows debounces detail loads by 150 ms.

Row status comes from events on every computer, plus lazy `conversation(id)`
and `run_execution` calls for rows as they appear. At most 4 calls per
computer are in flight; they can be cancelled and are fenced by cursor. An
unknown status shows no glyph.

### Toolbar and title

The leading side holds the title and subtitle:

- `navigationTitle` shows the task title, "New Task", the project name or
  "Jet Trash".
- `navigationSubtitle` shows "web-app · Claude Code". The assistant appears
  only when known, and "· Studio Mac" only for remote tasks. A project shows
  its path instead.
- `toolbarTitleMenu` holds `RenameButton()` with `.renameAction`, Show Working
  Copy in Finder (local tasks only) and Move to Jet Trash….

The principal item is a `TaskStatusLabel`, for example "Working · Editing
files" with a spinner. It is not a button, uses
`.sharedBackgroundVisibility(.hidden)` and has the lowest visibility priority,
so it truncates first.

The trailing side has two groups separated by `ToolbarSpacer(.fixed)`.

- Keep Changes… is the text group. It is present when the task has a Run and
  Git is available, and `.hidden(true)` otherwise. It uses `.glassProminent`
  when changes are known and `.glass` otherwise. During a reply it is disabled
  with the help text "Available when Claude Code finishes the current reply."
- The symbol group holds:
  - Interrupt (`stop.fill`, labelled "Interrupt", help "Interrupt the current
    reply (⌘.)"), shown only during a reply;
  - More (`ellipsis.circle`) with Rename…, Repeat Daily…, Task Settings…, Show
    Working Copy in Finder, Copy Working Copy Path, Stop Assistant… and Move to
    Jet Trash…;
  - Details (`sidebar.right`, "Show Details" or "Hide Details", ⌥⌘0). It is
    disabled on New Task and Jet Trash, with the help text "Details appear once
    a task starts."

There is no Settings button, no tint and no customization. Nothing overflows at
900×600 with Details open.

| State | Status | Trailing items |
|---|---|---|
| New Task | none | Details (disabled) |
| Working | Working · phase | Keep (disabled), Interrupt, More, Details |
| Waiting for your reply, with changes | Waiting for your reply | Keep (prominent), More, Details |
| Needs permission | orange Needs permission | Keep (disabled), Interrupt, More, Details |
| Offline | Offline · showing saved view | Keep (disabled), More, Details |
| Project page | none | More: New Task in web-app, Show in Finder, Move Project Folder to Trash… |

At most one banner shows, in the detail's top `safeAreaBar`:

- "Not connected to This Mac. Showing the last saved view." with Try Again;
- "Jet paused changes to protect your data." with Review…;
- "Your Mac is almost out of disk space. Jet paused new work." with Storage
  Settings….

Jet shows no alerts at launch. VoiceOver announces status changes of the
selected task at most once every 2 seconds.

### Transcript

The transcript is a `ScrollView` with a `LazyVStack`, at most 760 points wide,
with `.defaultScrollAnchor(.bottom)` and `.textSelection(.enabled)`. It follows
new output only while scrolled to the bottom; otherwise a floating Jump to
Latest button (`arrow.down`, `.glass`) appears. Streaming never moves focus.
Reduce Motion turns off animated scrolling. Text size comes from
`jet.transcript.text-scale`.

Entries:

- You. The label, time and text. The context menu has Copy and Edit as New
  Message, which fills the composer and never sends.
- The assistant. Labelled with the assistant's name, or "Assistant", never
  "Jet". Markdown renders inert through
  `AttributedString(markdown:options: .inlineOnlyPreservingWhitespace)`, with
  `.link` and `.imageURL` stripped. Headings, bullets, numbered lists and `[x]`
  checkboxes render. Code blocks show a language label and a Copy button
  (`document.on.document`).
- Steps. Consecutive tool names and activity lines fold into one
  `DisclosureGroup`, reading "Working… · Editing files" while live and "Used 3
  tools · Read, Edit, Bash" when done. Background-update counts, raw lifecycle
  lines and unknown blocks appear only with View › Show Technical Activity.
- Changes. A `change.checkpoint_recorded` event adds "Claude Code changed files
  in the working copy." with a green `checkmark.circle`. Review Changes opens
  Details › Changes for that reply, using the event's turn and Run. Keep
  Changes… appears on the latest row only, and only when no reply is running.
- Status rows. "Interrupted." · "Stopped. Send a message to continue." ·
  "Stopped with an error." with Send Again and Details · "Stopped unexpectedly.
  Send a message to continue." in neutral style.
- Queued messages. The You entry shows "Waiting to send · 2nd". Remove appears
  only when the entry is `withdrawable`, and puts the text back into an empty
  composer.
- The permission card and the closing Keep status line, described below.

History comes from an in-memory cache of 32 tasks × 256 entries, evicting the
least recently used task and fed by every computer's stream. A bounded journal
replay fills in older history: at most 10 windows of 2,000 events, 16 MiB and 8
seconds. It stops at the task's creation, is cancelled when the selection
changes, and shows determinate progress ("Loading earlier messages…"). If the
replay falls short, a card says "Earlier messages aren't available on this
Mac." with "Started Sep 26 · Last run ended Sep 27" and Review Changes. A task
with Runs never shows "Waiting for the first update". A task with no Runs shows
"No messages yet".

VoiceOver has a Messages rotor.

### Composer

In a task the composer is a `safeAreaBar(edge: .bottom)`, 760 points wide. On
New Task it is inline and 640 points wide. It is flat: a text background, a
1-point separator stroke, radius 8 and a 2-point copper focus ring. It has no
glass.

An `InlineNotice` above the field shows a symbol, a color, text and at most one
button. Confirmations clear on the next edit.

The field is `TextField(axis: .vertical)`, 1 to 8 lines (3 to 8 on New Task),
with the accessibility label "Task message".

| Situation | Placeholder |
|---|---|
| New Task | "Describe the change, bug, or question…" |
| Idle | "Reply to Claude Code…" |
| Working | "Send a follow-up…" |
| After Interrupt and Reply | "Tell Claude Code what to do instead…" |

Keys (ledger C3):

- ⌘↩ always sends. On macOS the menu owns this shortcut.
- Return inserts a newline by default. With Settings › General › "Press Return
  to Send" on, Return sends and Shift-Return inserts a newline.
- Jet never sends while marked IME text is present.

The leading controls depend on the context. New Task has `Picker(.menu)`
controls for the project (folder icon, grouped by computer, "Add Project…"
last), the assistant, and the computer when there are two or more. Changing the
computer shows "Project changed to web-app on Studio Mac". A task with no Runs
yet shows only the assistant picker. Any other task shows no leading controls.

The trailing controls are Interrupt (`stop.fill`, bordered, shown during a
reply) and Send or Start Task (`arrow.up`, `.borderedProminent`, identifier
`send-task`). The button reads "Starting…" or "Sending…" only during the
person's own action.

The caption shows the first of these that applies:

1. a blocker, with its fix;
2. "Sends after the current reply finishes.";
3. "Claude Code continues where it left off.";
4. on New Task, "Jet works in a separate copy of web-app. Your folder doesn't
   change until you keep the changes."

The trailing side of the caption shows "⌘↩ Send". Above 80% of the size limit
it shows "61,440 of 65,536 bytes" instead, turning red when over.

One `sendBlocker` feeds both the button and the menu item.

| Blocker | Message | Action |
|---|---|---|
| Not connected | "Not connected to This Mac. Your message is kept." | Try Again |
| Getting ready | "Jet is getting ready on this Mac…" | none |
| No project | "Choose a project to start." | Add Project… |
| No assistant | "Install Claude Code or Codex to start." | Check Again |
| Queue full | "Too many messages are waiting. Remove one or wait." | none |
| Too long | "Message is too long." | none |
| Git step unconfirmed | "Jet paused this task until you check a Git step." | Review… |

A task that already has a Run always continues with `submit_turn`. The core
relaunches the pinned assistant and resumes its native conversation.
`start_run` is used only when the task has no Runs. The assistant cannot be
switched on continuation.

After the first successful start, Jet asks once: "Get a notification when a
reply is ready or a task needs you?" with Turn On and Not Now.

### New Task and first run

New Task is a 640-point column with the JetMark at 32 points, "What are you
working on?" at 24 points, and "Describe what you need. You review the changes
before anything reaches your project."

While `!canStartTask`, an inline checklist shows three items.

- Jet on this Mac.
  - Starting: a spinner and "Starting Jet…". Shown once: "Jet runs a small
    helper in the background so tasks keep going when this window is closed."
  - Reconnecting: "Reconnecting… (attempt 2 of 3)", shown only while a retry is
    really scheduled, at 2, 5 and 15 seconds.
  - Failure: "Jet couldn't start its helper.", the mapped reason, Try Again,
    and a Details disclosure with the code and Copy.
    `core.install_incomplete` adds Reinstall Jet….
    `core.owned_by_other_channel` reads "Another copy of Jet already runs this
    Mac's helper." `core.start_failed` shows the generic failure.
  - Done: "Jet is running".
- Project: "Choose the Git repository you want to work on." with Add Project…
  and "or drop a folder here".
- Assistant: "No coding assistant found. Jet works with Claude Code and
  Codex." with Installation Help and Check Again.

Sign-in is optional and never blocks a first task.

While the draft is empty, examples insert editable text and never send it:
"Explain how this project is organized", "Fix a failing test" and "Find the
cause of a bug".

A folder drop, ⇧⌘O or Add Project… opens the Add Project sheet. It picks a
folder with `fileImporter`, then previews it:

- a Git repository: "Add “web-app”?" with the path, Cancel and Add Project;
- a subfolder: "This folder is inside “web-app”." with Use web-app;
- not Git: "“Downloads” isn't a Git repository. Jet needs Git so your changes
  can be kept safely.";
- a remote computer: a "Folder path on Studio Mac" field.

The confirmation stays because a Project cannot be unregistered.

### Details inspector

Details has a segmented control with Changes, Terminal and Activity (⌥⌘1 to
⌥⌘3), defaulting to Changes. `WorkPanelTab` raw values stay: `files` is
Changes in edit mode, and `run` is titled Activity. There is no in-panel header
or Close button. Details opens only on request; Interrupt and Stop never open
it.

Changes:

- Scope. A segmented "All Changes | Last Reply" control plus an "Earlier…"
  menu with Reply 1 to N, "When Claude Code stopped" and "Custom Range…" (a
  popover with From and To steppers). Changes apply immediately. If the task
  has several Runs, a caption reads "Showing changes since Claude Code last
  started for this task."
- Summary. "3 files changed", plus "+42 −7" once fully loaded, or "at least"
  when truncated.
- File rows. A symbol and a word, with the path truncated in the middle: Added
  (`plus.circle`, green), Modified (`pencil.circle`, no tint), Deleted
  (`minus.circle`, red).
- Diff. Clicking a row expands that file's hunks, split from the patch at
  `diff --git`. It uses monospaced 12-point text with a +/− gutter, Increased
  Contrast tints, and horizontal scrolling only. A binary file shows "Binary
  file changed". A truncated file shows "This file's changes are cut off." with
  Load More.
- The file context menu has Edit File, Comment on Line…, Copy Path, and Show
  in Finder for local tasks.
- Edit mode. The header reads "login.ts — Edited", with Save (⌘S) bound to the
  file's revision, Revert… ("Discard your edits to login.ts?") and Done. The
  unsaved-edit guard applies.
- Comment on Line… opens a popover with Line and Comment fields. The comment is
  sent to the assistant as a message.
- The footer is a `safeAreaBar` with the Keep status line and Keep Changes….
- Empty and error states: "No Task Selected", "No Changes Yet · Changes Claude
  Code makes appear here.", "Loading changes…", and an unavailable view with
  Try Again.
- VoiceOver has a Changed Files rotor.

Terminal has New Terminal, a picker of "Terminal 1…", output and input, Attach
or Detach, and Close Terminal. The terminal UUID and state move into a tooltip.
The empty state reads "Terminals open in this task's working copy."

Activity has these sections:

- Now: the status and how long it has lasted, with Interrupt… and Stop
  Assistant….
- Waiting to Send: excerpts joined from the transcript, or "A scheduled
  message" or "Message from another Jet app". Remove appears when the entry is
  withdrawable.
- Working Copy: the path, Show in Finder and Copy Path.
- Git Activity: plain sentences, with Try Again…, Mark as Checked… and Check
  Status.
- Technical Details, a disclosure with lifecycle and activity; Run ID,
  revision, turn and checkpoint; the termination summary; queue limits;
  `selectedPlaneName`; and the task ID with Copy.

### Keep Changes

Keep Changes is a sheet presented at the shell root. Its subtitle reads "Claude
Code changed 3 files in a separate working copy. Your web-app folder hasn't
changed."

The plan is a `.radioGroup`:

- Save to a branch (default): "Commits the changes to a new branch in web-app.
  Your checked-out files don't change."
- Save and push: "Also pushes the branch to origin."
- Open a draft pull request: "Also opens a draft pull request on GitHub." It
  always shows "Needs a GitHub token saved for Jet in your Keychain." with How
  to Set Up….

The branch name is prefilled with the branch prefix plus a slug of the task
title and is validated as the person types. If a branch step already
completed, it reads "Uses branch jet/… created earlier." An Options disclosure
holds Remote and Base branch. A step list shows exactly the steps that will
run, each with its state: Create branch, Commit (Jet writes the message), Push
to origin, Open a draft pull request. The buttons are Cancel and a primary
button matching the plan: "Save to Branch", "Save and Push" or "Create Pull
Request". The primary keeps the identifier `git-delivery-review`.

The sheet confirms the whole chosen chain once (ledger C2). Confirming closes
the sheet. A `DeliveryCoordinator`, keyed by computer and task, then submits
one `DeliverGit` Command per step. Each Command has its own retained command
ID and exact body, is pinned to the reviewed checkpoint, and starts only after
the previous step is `completed`. The chain stops at a failure, at "couldn't
confirm" or at an uncertain admission, and never retries. Progress shows in
the Keep status line in the Details footer and at the end of the transcript.
After a relaunch, steps that were never submitted read "Not started".

| Outcome | Presentation |
|---|---|
| Completed | "Saved to branch jet/fix-login-redirect" with Copy Branch Name, and "To use it in your project: git switch jet/fix-login-redirect" with Copy. A pull request adds "Draft pull request opened" with Open Pull Request, shown only for https github.com URLs. |
| Failed | A sentence mapped from the code, with its fix. Unknown codes read "Git refused this step." Try Again… retries only the failed step, after confirmation. |
| Couldn't confirm | "Jet couldn't confirm whether the push happened. Check the branch on GitHub, then mark it as checked." with Check Status and Mark as Checked…. The confirmation says "Jet won't retry or undo it." Until then the task is in Needs You and the composer is blocked. |
| Uncertain admission | "Jet couldn't confirm it received the request." with Check Status and Send Same Request Again. |

The Changes menu's single steps open the same sheet in single-step mode. This
sheet never enables automatic delivery.

### Permission, Interrupt and Stop Assistant

The permission card is content with an orange leading rule. It has no glass and
no Approve button.

- Title: `hand.raised.fill` "Claude Code is waiting for permission".
- Body: "It wants to run: `make test`" and "Jet can't grant permissions from
  the app yet, so this reply is paused." The command is extracted for display
  only, falling back to the tool name.
- Primary action: Interrupt and Reply…. It shows the Interrupt confirmation,
  then focuses the composer with "Tell Claude Code what to do instead…".
- A secondary menu has Copy Command, Open Terminal and Stop Assistant….
- A Details disclosure shows Target, Scope, Consequence, the requested action
  in selectable monospace, and the rationale.
- When a request is denied and a retry is allowed, a bordered Ask Reviewer
  Again button shows with "The same safety checks apply; it may be blocked
  again."
- Resolved cards collapse to one line: a green `checkmark.shield` "Allowed by
  the safety review: make test", or a red `xmark.shield` "Blocked by the safety
  review: make test".

Automatic review is never offered as the way out. Controls appear only on the
latest pending card.

Interrupt (⌘.) asks "Interrupt Claude Code?" with "Claude Code stops its
current reply. Messages and changes are kept, and waiting messages go next."
Interrupt is the default button, with Cancel. ⌘. is disabled while any sheet or
dialog is open.

Stop Assistant… has no shortcut. It asks "Stop Claude Code for this task?" with
"Claude Code quits, including commands it started (Stop Run). Messages and
changes are kept. Send a message to start it again." The buttons are Stop
Assistant (destructive) and Cancel (default).

Both dialogs live at the shell root.

Blocking notices above the composer cover sign-in ("Sign in to Claude Code on
this Mac, then send a message to continue." with Assistant Settings…) and quota
("Usage limit reached. Send a message after it resets." with Usage…).

### Projects, Jet Trash and task sheets

The project page is a grouped `Form`:

- Tasks in This Project, with New Task in web-app.
- When a Reply Finishes: Do Nothing, Save to a Branch, Save and Push, or Save,
  Push and Open a Draft Pull Request. The preset writes the `git.auto_*`
  settings in policy order, and in reverse order when turning them off.
  Presets that push or open a pull request first ask "Push automatically after
  every reply? Jet pushes each reply's changes to origin without asking."
  Mixed values show "Custom".
- "Branch names start with" and "Name tasks automatically".
- Location: the path, Show in Finder and Copy Path.

Move Project Folder to Trash… appears only in the project context menu and
More. The reviewed sheet stays with three changes: Cancel is the default
button, the copy says "macOS Trash", and the title reads "Move the
“api-server” folder to the Trash?".

The Jet Trash page is a `Table` with the columns Task, Project, Deleted and
Removed On. Restore takes one click (`trash-restore-<id>`). Titles come from
`conversation(id)`, then the loaded list, then `jet.trash-titles.v1`, then
"Untitled task". The footer reads "Tasks are removed for good after N days.",
with N from `retention.trash_grace_days`. The empty state reads "Jet Trash Is
Empty".

Move to Jet Trash… is a sheet using `JetRecoveryModel` and the retention
preview.

- Title: "Move “Fix login redirect” to Jet Trash?"
- Message: "You can restore it from Jet Trash until Oct 28. After that its
  working copy and Jet history are deleted. Claude Code's own history isn't
  deleted." The date always comes from the preview.
- Protections, as sentences:

  | Protection | Sentence |
  |---|---|
  | `dirty_workspace` | "Its working copy has changes that aren't saved to a branch." |
  | `unpushed_work` | "It has commits that weren't pushed." |
  | `unresolved_effect` | "A Git step couldn't be confirmed." |
  | `enabled_schedule` | "Its daily message stops." |
  | any other code | shown under Details |

- Buttons: Cancel (default) and Move to Jet Trash.
- `active_run`, the common case: the title becomes "Claude Code is still open
  for this task." and the primary becomes Stop and Move to Jet Trash
  (destructive). It stops the Run, waits up to 30 seconds for it to end, checks
  again, then forgets the task.
- `pending_turn`: "Messages are still waiting to send." with Show Waiting
  Messages.
- The secondary action is Delete Everywhere….
- Afterwards the next task is selected, and an Undo notice offers to restore.

Repeat Daily… is a sheet with Message (up to 8,192 bytes), At
(`DatePicker(.hourAndMinute)`, wall-clock time in the chosen zone), and Time
Zone (the current zone, with a searchable list). Existing schedules read
"Every day at 9:00 (Berlin)", each with a confirmed Stop Repeating….

Task Settings… holds this task's When a Reply Finishes, including "Use Project
Default (…)".

### Settings

Settings is `Settings { TabView }` with a grouped `Form` in each tab. The
`jet.settings.last-pane` raw values stay.

Settings never follows the main window's selection. With two or more
computers, each pane starts with "Settings for: [This Mac ▾]". When the
computer is offline, one banner reads "Can't reach This Mac. Settings can't be
changed right now." with Try Again. Toggles and pickers apply as soon as they
change. Numbers apply on Return or when focus leaves the field. Each row shows
its own state. "Reset to Default (Off)" replaces "Use Inherited Value". Error
codes appear only under Details.

| Tab (raw value) | Content |
|---|---|
| General (`general`) | "When Jet opens: Reopen the last task". "Press Return to Send", off by default. Text size. Notify Me When: "A task needs you", "A reply is ready", "A task stops with an error", keeping the `notifications-*` identifiers. When permission is denied, a note and Open System Settings…. The appearance rows are removed; the app follows the system appearance. |
| Assistants (`agents`) | "Installed · Using your existing sign-in ✓", or Use Existing Sign-In. Remove Sign-In…, confirmed. A usage `Gauge`: "42% of 5-hour limit · Resets 14:30". |
| Tasks (`work`) | "Keep deleted tasks in Jet Trash for [30] days", with Show Jet Trash. Clean Up Idle Tasks keeps the compile-and-approve review and lists candidates by title. |
| Computers (`connections`) | See below. |
| Safety (`safety`) | Automatic safety review and the reviewer. "Let Claude review requests from Codex tasks (request details are sent to Anthropic)", confirmed the first time. Work limits: "Run up to [N] tasks at once". |
| Advanced (`advanced`, new) | Service and versions. Recovery snapshots. Storage in MB or GB. Diagnostics with Copy Diagnostic Summary. Security Audit. Extensions. Developer Mode. |

The Computers tab is the only home for computers.

- This Mac shows its status and Allow Another Mac to Connect…. That sheet
  shows the code and a QR code, polls every 2 seconds, and asks "Check that
  both computers show: river-paper-sunset" with Codes Match and Don't Match.
- Connected Jet Apps lists each app with "Paired Aug 12" and a confirmed
  Revoke….
- Each remote computer card has Reconnect and Remove from This Mac…, confirmed
  with "Its tasks stay on Studio Mac but won't appear here until you connect
  again."
- Connect Another Computer… is a three-step sheet: get a code on the other
  computer; enter the name, SSH address and code; check that the words match.
  It ends with "New tasks still start on This Mac". Pairing never switches the
  default computer.

`JetSettingsPane.resolving` keeps its five pinned routes and adds `storage`,
`audit` and `recovery`, which resolve to `.advanced`.

### Notifications and Dock

Notifications carry no task content. The body is always "Click to open the task
in Jet."

| Title | Interruption level | When |
|---|---|---|
| "Task Needs You" | active | the task needs permission |
| "Reply Ready" | passive | `waiting_for_user` or `completed`, at most once per reply |
| "Task Stopped" | active | `failed`, or `lost` observed live |

`userInfo` holds only the task and computer IDs. A
`UNUserNotificationCenterDelegate` opens the task. Jet sends notifications only
while it is in the background and deduplicates them by event ID. The Dock
badge counts Needs You tasks.

### Menu bar

Items that do not apply stay visible and dimmed.

| Menu | Items |
|---|---|
| Jet | About Jet · Settings… ⌘, · Services · Hide Jet ⌘H · Hide Others ⌥⌘H · Show All · Quit Jet ⌘Q |
| File | New Task ⌘N · New Task in Project ▸ · Add Project… ⇧⌘O · Close Window ⌘W |
| Edit | system items · Find ▸ Find Task… ⌘K |
| View | Show/Hide Sidebar ⌃⌘S · Show/Hide Details ⌥⌘0 · Changes ⌥⌘1 · Terminal ⌥⌘2 · Activity ⌥⌘3 · Show Jet Trash · Show Technical Activity ✓ · Bigger ⌘+ · Smaller ⌘− · Actual Size ⌘0 · Enter Full Screen ⌃⌘F |
| Task | Start Task / Send ⌘↩ · Interrupt… ⌘. · Stop Assistant… · Rename… · Repeat Daily… · Task Settings… · Show Working Copy in Finder · Copy Working Copy Path · Move to Jet Trash… ⌘⌫ · Delete Everywhere… (with ⌥) |
| Changes | Review Changes · Keep Changes… ⇧⌘K · Create Branch… · Commit… · Push… · Open Draft Pull Request… · Check Git Status · Mark as Checked… |
| Window, Help | system items · Jet Help · How Jet Keeps Your Project Safe (both open the docs in the browser) |

Task replaces the Conversation menu, and Changes replaces Delivery. One
`canSend` feeds both the menu item and the button. ⌘⌫ is disabled while a text
input has focus, through the `isEditingText` focused value; the sidebar Delete
key alone is the fallback. ⌘. is disabled while a sheet or dialog is open.

### Context menus

Only the items that apply are shown. Within a group, either every item has an
icon or none does.

| Target | Items |
|---|---|
| Task row | Rename… (`pencil`), Repeat Daily… (`clock`) · Show Working Copy in Finder (`folder`), Copy Branch Name (`document.on.document`) · Move to Jet Trash… (`trash`) |
| Project row | New Task in “web-app”, Show in Finder, Copy Path · Move Project Folder to Trash… |
| Your message | Copy, Edit as New Message |
| Assistant message | Copy |
| Code block | Copy Code |
| Changed file | Edit File, Comment on Line…, Copy Path, Show in Finder |
| Jet Trash row | Restore |

### Keyboard, focus, pointer and drag and drop

- Full Keyboard Access covers the whole loop. The Tab order is sidebar,
  transcript, composer, inspector.
- Focus moves to the composer after ⌘N and after Return on a row, stays in the
  composer after Send, and never moves because of streaming.
- Icon controls have verb-first tooltips that match their labels and a
  minimum hit area of 28 points.
- Dropping a folder on New Task or Projects opens Add Project, through
  `dropDestination(for: URL.self, isEnabled:)`. Dropping files into a prompt
  is not supported.
- Text is selectable. Code, branches, paths and commands also have Copy
  buttons.
- The Move to Jet Trash notice offers Undo.

## Mac design language

Content comes first, with one primary action per state. Spacing follows 4, 8,
12, 16, 24 and 32. Sections are separated by space or separators, never by
nested cards.

| Type role | Size in points |
|---|---|
| Metadata | 11 |
| Controls | 12 |
| Navigation | 13 |
| Transcript, scaled 0.85 to 2.0 | 14 |
| Titles | 17 |
| New Task invitation | 24 |
| Code | monospaced 12 to 13 |

Nothing is smaller than 10 points, and no light weights are used.

| Color | Used for |
|---|---|
| Copper (`AccentColor`) | the single prominent action, system selection, the composer focus ring, progress, the unread dot |
| Orange | needs you, warnings |
| Green | success, added lines |
| Red | errors, removed lines, destructive actions |

Color is never the only signal.

Copper is forced with `.tint(JetDesign.accent)` on the main window and Settings
roots (ledger C5). The HIG says a person's non-multicolor accent setting
replaces an app's accent color; Jet deliberately keeps copper so the visual
gamut stays.

Copper comes from two tokens (ledger C4):

| Token | Role | Light | Dark | Light, Increased Contrast | Dark, Increased Contrast |
|---|---|---|---|---|---|
| `AccentColor`, through `JetDesign.accent` | copper fills: prominent buttons, selection, focus ring, progress | #A0472B | #C15634 | #873B23 | #A94B2D |
| `JetDesign.accentText` | copper text and glyphs on plain surfaces | #A0472B | #E3A080 | #873B23 | #EDB597 |

The dark fill #C15634 keeps the light hue and gives white labels 4.5:1
contrast. The diff tints also have Increased Contrast variants.

Liquid Glass comes only from system bars, the sidebar and the inspector;
sheets, menus and popovers; and the floating Jump to Latest button. Content is
never glass. Test with Reduce Transparency, Increase Contrast and each glass
look.

Icons are the SF Symbols named in each section: hierarchical for status,
monochrome elsewhere. Menu items get no custom icons.

Motion uses system spinners and transitions only. There are no symbol effects
and no typewriter streaming. Reduce Motion stops animated scrolling and row
moves.

Custom components are allowed only where system UI cannot express the content:
the composer container, the permission card, the diff view, the status label
and the inline notice. Each keeps focus rings, 28-point targets, labels and
appearance variants.

## Status vocabulary

On macOS one derivation feeds sidebar rows, the toolbar, Activity,
notifications and VoiceOver. Priority is offline, then Needs You, then in
progress, then failed, then the rest. Lifecycle and activity stay separate
inputs (ADR-0065). An unknown status shows nothing.

| Input | Label | Symbol / tint | Row glyph | Needs You |
|---|---|---|---|---|
| cached or unreachable | Offline · showing saved view | `wifi.slash` / secondary | dimmed | no |
| `waiting_for_approval` | Needs permission | `hand.raised.fill` / orange | yes | yes |
| `waiting_for_auth` | Sign-in needed | `person.crop.circle.badge.exclamationmark` / orange | yes | yes |
| `waiting_for_quota` | Usage limit reached | `hourglass` / orange | yes | yes |
| unacknowledged unknown Git outcome | Couldn't confirm a Git step | `questionmark.circle` / orange | yes | yes |
| `created`, `starting` | Starting… | spinner / accent | spinner | no |
| `active` and working | Working · phase | spinner / accent | spinner | no |
| reconnecting, `stopping` | Reconnecting… / Stopping… | spinner / secondary | spinner | no |
| `failed` | Stopped with an error | `xmark.octagon.fill` / red | yes | no |
| `active` and `waiting_for_user` | Waiting for your reply | `bubble.left` / secondary | unread dot only | no |
| `completed` | Finished · send a message to continue | `checkmark.circle` / secondary | unread dot only | no |
| `canceled`, `lost` | Stopped · send a message to continue | `stop.circle` / secondary | none | no |

The unread dot is a 7-point copper `circle.fill`, shown when the latest reply
sequence is newer than the last one seen.

The phase is display-only, mapped from the latest tool name:

| Tool | Phase |
|---|---|
| Edit, Write, MultiEdit | Editing files |
| Bash | Running a command |
| Read, Grep, Glob, LS | Reading the project |
| WebFetch, WebSearch | Searching the web |
| anything else | Working |

`lost` after a reboot stays neutral. It is never red and never in Needs You.

## State matrix

| Screen | Empty | Loading | Offline | Error | Destructive or recovery |
|---|---|---|---|---|---|
| Sidebar | "Tasks you start appear here." | redacted rows | offline row with Try Again; rows dimmed | "Couldn't load tasks" with Try Again | Move to Jet Trash (Delete); Undo notice |
| New Task | examples | checklist with the real retry count | blocker; draft kept | mapped helper error with Details | Try Again, Reinstall Jet… |
| Transcript | "No messages yet" | determinate history load | banner; Commands disabled | status row with Send Again | permission card; summary card |
| Composer | Send disabled | Sending…, for the person's own action only | blocker | notice with fix | Interrupt confirmation; draft kept |
| Details | "No Changes Yet" | "Loading changes…" | cached and labelled | Try Again | unsaved-edit alert; Revert…; Load More |
| Keep Changes | "No changes to keep yet." | per-step spinner | disabled with reason | mapped failure with Try Again… | step review; Mark as Checked…; Send Same Request Again |
| Jet Trash | "Jet Trash Is Empty" | redacted rows | banner | inline issue | Delete Everywhere review; Restore |
| Settings | per pane | "Loading…" on the row | one banner | inline on the row | confirmed revoke, remove and purge; Recovery; Open System Settings… |

## Trust, privacy and accessibility

- Jet never manufactures success.
- Before the person chooses, Jet says what leaves the Mac (a push, a pull
  request, a cross-provider review) and what a draft pull request needs.
- Automatic delivery needs its own confirmation.
- Notifications carry no content.
- Persisted client memory holds only IDs, sequences and Jet Trash titles.
- Markdown stays inert. Tests assert that no link or image attribute survives,
  and the ASVS comments move with the code.
- Status is always a symbol plus text. Icon labels equal their tooltips.
- Interrupt is labelled "Interrupt", never "Stop". Stop Assistant never uses
  `stop.fill`.
- Rotors cover Messages and Changed Files.
- Text scales to 200%. Metadata is at least 10 points. Dimmed rows still meet
  4.5:1 contrast.
- Diffs use +/− markers, not color alone.
- Reduce Motion, Reduce Transparency and Increase Contrast are honored.
- Copy lives in String Catalogs, with no concatenated sentences.
- `#if os(macOS)` seams stay narrow so the iOS target keeps compiling.

## Changes from the previous macOS design

The September 26 record described one work model for both apps. The macOS
redesign replaces the following parts of it for `apps/jet`.

| Previous macOS design | Redesign | Why |
|---|---|---|
| Projects, Schedules, Planes, Trash and Settings as secondary destinations | Sidebar with New Task, Needs You, Tasks, Projects and Jet Trash; computers only in Settings › Computers; schedules per task through Repeat Daily… | Each object has one obvious home, and attention items surface in the list people already watch |
| Work panel with Changes, Files, Terminal, Run and Delivery | Details inspector with Changes, Terminal and Activity; Files becomes Changes in edit mode | Five segments truncate at the 300-point inspector width |
| Delivery menu and panel with individual Git steps | One Keep Changes sheet that confirms a whole plan; single steps stay in the Changes menu | Casual users need one path to keep their work, and the outcome must name the branch |
| Modal inspector in narrow windows | Non-modal inspector that auto-collapses the sidebar below 1100 points | A modal inspector blocks the conversation |
| A setup step presenting Project and Harness | An inline checklist on New Task, shown only while a task cannot start | Setup happens where the first task starts, and nothing else stands between the person and that task |
| Domain words such as Harnesses, Delivery and Connections in navigation and Settings | A fixed lexicon that maps each domain term to one interface word | Casual users should never need Jet's internal vocabulary to find a control |
| Return always inserts a newline | ⌘↩ sends by default, with an optional "Press Return to Send" (ledger C3) | Keeps IME safety while giving chat-style sending to people who want it |
| Dark copper fill #E3A080 | Dark fill #C15634, with #E3A080 kept as `JetDesign.accentText` (ledger C4) | White labels on the old dark fill do not reach 4.5:1 |
| Settings tabs General, Harnesses, Work, Connections, and Safety & System | General, Assistants, Tasks, Computers, Safety, Advanced | Tab names use interface words, and expert items move to Advanced |
| Appearance rows in Settings | Removed; the app follows the system appearance | The system already owns this choice |

Rejected alternatives that shaped the design:

| Alternative | Why it was rejected |
|---|---|
| Automatic review as the way out of a permission request | The v2 reviewer admits only `pwd`, `echo` and `true` |
| An automation checkbox in the Keep sheet | Publishing needs its own review |
| A full journal scan for history | Unbounded |
| A permanent "aren't shown" for old history | History should stay reachable |
| Switching the assistant on continuation | Loses the assistant's native context |
| Sorting tasks by activity | Activity is display-only and not on the wire |
| "Messages" sections in search | There is no content index |
| Send turning into Interrupt during a reply | ⌘↩ would interrupt |
| Return to send by default | IME risk (ledger C3) |
| "Force Stop" as a label | Reads like Force Quit |
| Five inspector segments | Truncate at 300 points |
| ⌥⌘. for Stop Assistant | One modifier away from ⌘. |
| Dialogs anchored to their source control | Not a macOS pattern |
| A modal compact inspector | Blocks the conversation |

## Platform evidence

Apple sources were accessed on September 28, 2026. The redesign needs no
deployment target change and no beta-only API.

| Choice | Guidance | Jet consequence |
|---|---|---|
| Library window with sidebar and inspector | [Designing for macOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos), [Windows](https://developer.apple.com/design/human-interface-guidelines/windows), [Split views](https://developer.apple.com/design/human-interface-guidelines/split-views), [Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars), [Lists and tables](https://developer.apple.com/design/human-interface-guidelines/lists-and-tables) | One main window, native selection, sidebar auto-collapses when Details needs room |
| Selection and restoration | [Focus and selection](https://developer.apple.com/design/human-interface-guidelines/focus-and-selection), [Launching](https://developer.apple.com/design/human-interface-guidelines/launching) | Refresh never steals selection; launch restores the last task |
| Toolbar and materials | [Toolbars](https://developer.apple.com/design/human-interface-guidelines/toolbars), [Layout](https://developer.apple.com/design/human-interface-guidelines/layout), [Adopting Liquid Glass](https://developer.apple.com/documentation/technologyoverviews/adopting-liquid-glass) | Grouped trailing items, glass only from system components |
| Details on demand | [Panels](https://developer.apple.com/design/human-interface-guidelines/panels), [Segmented controls](https://developer.apple.com/design/human-interface-guidelines/segmented-controls), [Modality](https://developer.apple.com/design/human-interface-guidelines/modality), [Disclosure controls](https://developer.apple.com/design/human-interface-guidelines/disclosure-controls) | Three-segment non-modal inspector; Technical Details behind disclosure |
| Composer and pickers | [Text fields](https://developer.apple.com/design/human-interface-guidelines/text-fields), [Pop-up buttons](https://developer.apple.com/design/human-interface-guidelines/pop-up-buttons), [Scroll views](https://developer.apple.com/design/human-interface-guidelines/scroll-views) | Multi-line field, menu pickers, follow-bottom scrolling |
| Assistant output and control | [Generative AI](https://developer.apple.com/design/human-interface-guidelines/generative-ai) | People stay in control; significant actions are confirmed |
| Confirmations and undo | [Sheets](https://developer.apple.com/design/human-interface-guidelines/sheets), [Alerts](https://developer.apple.com/design/human-interface-guidelines/alerts), [Undo and redo](https://developer.apple.com/design/human-interface-guidelines/undo-and-redo) | Keep Changes and Move to Jet Trash are sheets; Jet Trash has Undo |
| Progress and feedback | [Feedback](https://developer.apple.com/design/human-interface-guidelines/feedback), [Progress indicators](https://developer.apple.com/design/human-interface-guidelines/progress-indicators), [Loading](https://developer.apple.com/design/human-interface-guidelines/loading) | Spinners only for real work; determinate history loading |
| Search | [Search fields](https://developer.apple.com/design/human-interface-guidelines/search-fields), [Searching](https://developer.apple.com/design/human-interface-guidelines/searching) | Sidebar search with instant title filtering |
| Keyboard and menus | [Keyboards](https://developer.apple.com/design/human-interface-guidelines/keyboards), [The menu bar](https://developer.apple.com/design/human-interface-guidelines/the-menu-bar), [Context menus](https://developer.apple.com/design/human-interface-guidelines/context-menus), [Pull-down buttons](https://developer.apple.com/design/human-interface-guidelines/pull-down-buttons) | ⌘. interrupts, a full menu bar, dimmed inapplicable items |
| Folders and drop | [File management](https://developer.apple.com/design/human-interface-guidelines/file-management), [Drag and drop](https://developer.apple.com/design/human-interface-guidelines/drag-and-drop) | Folder drop opens Add Project |
| First run | [Onboarding](https://developer.apple.com/design/human-interface-guidelines/onboarding) | Inline checklist; remote setup stays optional |
| Settings | [Settings](https://developer.apple.com/design/human-interface-guidelines/settings), [Tab views](https://developer.apple.com/design/human-interface-guidelines/tab-views), [Toggles](https://developer.apple.com/design/human-interface-guidelines/toggles) | Six tabs; task options live with the task |
| Notifications | [Notifications](https://developer.apple.com/design/human-interface-guidelines/notifications), [Managing notifications](https://developer.apple.com/design/human-interface-guidelines/managing-notifications), [Multitasking](https://developer.apple.com/design/human-interface-guidelines/multitasking) | Opt-in, content-free, background only |
| Accent color | [Color](https://developer.apple.com/design/human-interface-guidelines/color) | Forced copper is a recorded deviation (ledger C5) |
| Copy and inclusion | [Writing](https://developer.apple.com/design/human-interface-guidelines/writing), [Inclusion](https://developer.apple.com/design/human-interface-guidelines/inclusion), [Accessibility](https://developer.apple.com/design/human-interface-guidelines/accessibility), [Offering help](https://developer.apple.com/design/human-interface-guidelines/offering-help) | Verb-first labels, empty states with a next step, never color alone |

Within the Apple ecosystem, the macOS app uses Finder (Show in Finder), the
Keychain (the GitHub token a draft pull request needs), user notifications and
the Dock badge. The iOS target keeps compiling and gets no redesign. Charts,
maps, media playback, purchases, cloud document sync and device continuity have
no role in this workflow. Spotlight, App Intents and widgets are deferred.

## Verification and capability audit

### Capability destinations

The audit used `CommandRequest`, `QueryRequest`, the clients, and the feature
documents below. A visible task is a durable Conversation, not a Run. The
Tauri column records the implemented September 26 interface. The macOS column
names where each capability lives in the confirmed redesign; it is the target,
not verified behavior. Implementation verification for macOS is added when the
redesign lands.

| Capability | macOS SwiftUI (redesign target) | Tauri desktop | Contract |
| --- | --- | --- | --- |
| Register and inspect a Project; reviewed removal | Projects sidebar section and project page, Add Project sheet with folder picker or drop, Move Project Folder to Trash… review | Projects, native folder picker and removal dialog | [Project removal](project-removal.md), `RegisterProject`, `PreviewProject` |
| Choose installed Harness and start isolated work | New Task with project, assistant and, with two or more computers, computer pickers | New task, Project/Harness selectors | [Managed Runs](managed-runs.md), `CreateConversation`, `StartRun` |
| Resume work, read history, search, rename | Sidebar Tasks and Needs You, sidebar search with Other Matches, title menu Rename | Task library, Find command and task actions | `Conversation`, `Search`, `SetConversationName` |
| Submit and withdraw queued messages | Composer, Waiting to send entries with Remove, Details › Activity | Composer and Run details | [Turn queue](turn-queue.md) |
| Interrupt a turn, stop a Run, resolve orphaned execution | Interrupt (⌘.) and Stop Assistant… confirmations; orphaned-execution UI deferred | Reviewed controls and Run details | [Execution control](execution-control.md) |
| Inspect approvals and request an allowed review retry | Permission card with Interrupt and Reply… and Ask Reviewer Again | Transcript approval details | [Automatic review](automatic-review.md), `AuthorizeApprovalRetry` |
| Inspect checkpoints, changed files, artifacts and diffs; edit files and send review comments | Details › Changes with reply scopes, edit mode and Comment on Line… | Changes and Files in work details | [Checkpoints](change-checkpoints.md), [edits and reviews](user-edits-and-reviews.md), [artifacts](artifacts.md) |
| Open, attach, detach and close Workspace terminals | Details › Terminal | Terminal in work details | [Workspace terminals](workspace-terminals.md) |
| Review branch, commit, push and GitHub draft PR operations; reconcile uncertain delivery | Keep Changes… sheet, Changes menu single steps, Check Status and Mark as Checked… | Delivery panel and native View menu | [Git delivery](git-delivery.md) |
| List, create and cancel daily schedules | Repeat Daily… sheet per task with Stop Repeating…; cross-task overview deferred | Schedules destination with task selector and confirmation | [Schedules](schedules.md), existing protocol minor 21 |
| Scoped defaults, Auto-continue and automatic review settings | Project page and Task Settings… (When a Reply Finishes), Settings › Safety; Auto-continue control deferred | Work and Safety Settings | [Auto-continue](auto-continue.md), `Settings`, `SetSetting`, `ClearSetting` |
| Account bindings, usage, Craft installation/disable and native extensions | Settings › Assistants for sign-in and usage; Crafts and Extensions in Settings › Advanced | Harnesses Settings | [Craft lifecycle](craft-lifecycle.md), [installation](craft-installation.md), [extensions](harness-extensions.md), [usage](usage-records.md) |
| Pair, inspect, disconnect and revoke access to remote computers | Settings › Computers | Connections and native View menu | [Remote connections](remote-connections.md) |
| Move to Jet Trash, delete everywhere with review, restore, manage grace period and Autodelete rules | Jet Trash page, Move to Jet Trash… sheet with Undo, Delete Everywhere…, Settings › Tasks | Trash and Work Settings | [Autodelete](autodelete.md) |
| Storage/recovery state, audit, diagnostics, service and version information | Settings › Advanced; recovery and disk banners | Safety and system; Connections | [Recovery](recovery.md), [diagnostics](diagnostics.md), [disk pressure](disk-pressure.md) |
| Notifications and presentation restoration | Settings › General › Notify Me When; scene and app storage restoration | Separate native Settings window and app-local presentation state | Existing opt-in notification and privacy contract |

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
entry in either app. This is an explicit exclusion, not a claim that a
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
  text, not attachment references. No decorative drop zone is shown. The macOS
  folder drop only adds a Project.

The general approval-request protocol does not expose an interactive
Approve/Reject Command. The transcript therefore reports the request and
available retry actions without manufacturing a decision. No-Visa tool review
is a separate protocol operation and is excluded above.

### Deferred macOS work

- Open in New Window.
- Toolbar customization.
- ⌘F find in the transcript.
- The Dock menu and TipKit.
- Persisted drafts.
- The Auto-continue control, which needs client wrappers and `HermeticJetd`
  handlers.
- An orphaned-execution UI.
- A Schedules overview across tasks.
- Spotlight, App Intents and widgets.
- An honest empty state on iOS.
- A copper app icon. The icon is still Jet Blue #29B6F6.

### Protocol gaps

The macOS redesign works around these gaps on the client. Closing them is
backend work outside this redesign.

1. History. There is no per-Conversation history query. The client uses a
   cache plus a bounded replay.
2. Summaries. Conversation summaries lack status, last-activity and
   unkept-changes fields. The client loads them lazily and never sorts by
   activity.
3. Craft. Runs don't expose their Craft. Assistant labels exist only for Runs
   this client started.
4. Approvals. There is no approve or deny Command.
5. Progress. There is no structured phase. The phase is mapped from tool
   names.
6. Git delivery. It emits no events. "Couldn't confirm" is known only after a
   query.
7. Repository facts. Jet can't check or store the GitHub token. Remote,
   default-branch and host facts are missing.
8. Queue and Trash. Queue entries have no excerpt. Trash entries have no title
   or project.
9. Search. Search covers names, paths and branches only.
10. Pairing. There is no event for a pairing claim, so the client polls.
11. Autodelete. A days-only rule still needs compiling.
12. Projects and schedules. A Project can't be unregistered, and there is no
    schedules query across Conversations.
13. Change scopes. Scopes are per Run. There is no diff for a whole
    Conversation.
14. Quota. Quota windows aren't linked to `waiting_for_quota`.

### Verification evidence

Checks performed on September 26, 2026 on macOS. The Swift checks below cover
the previous macOS interface, not this redesign.

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

#### Native macOS redesign (September 29, 2026)

The SwiftUI redesign was checked on macOS 27 with the Command Line Tools and the
macOS 26.5 SDK, building the app sources as a Swift package:

- The full `jetTests` suite passes, including the framed-protocol integration
  test and `CopyLintTests`, which scans the feature views' user-facing strings
  for the domain words in the lexicon's Avoid list. Its allowlist names the
  places the design permits them (Settings › Advanced, Connection Details and
  the Stop and Move to Jet Trash reviews).
- Every preview scene (139, most in light and dark) was rendered in a real window
  and reviewed. Two narrow-window layout loops found there were fixed: text with a
  fixed vertical size inside the composer's caption and the Details footer made
  `.contentMinSize` windows overflow or trap in AppKit constraint passes.
- Dates and numbers follow the interface language with the person's region
  (`JetCopy.uiLocale`), so an English interface on a Russian-region Mac reads
  "25 min. ago" and "21 Sep at 17:01".

Not verified by these checks: the Xcode-built app (menu key equivalents,
notification clicks, the Dock badge, reopening from the Dock, copper accent in
active windows), the iOS build, input-method composition in the composer, and
VoiceOver. The screenshot harness renders inactive windows, so prominent buttons
appear grey there.

## Decision ledger

### Confirmed decisions

The user confirmed these on September 28, 2026.

- C1. The macOS design in this record as a whole, including its scope limits
  and the visual gamut it keeps.
- C2. Keep Changes uses one review sheet that confirms the whole chosen chain.
  Each Git step stays its own `DeliverGit` Command, and the chain stops at the
  first failure or uncertain outcome.
- C3. ⌘↩ sends by default and Return inserts a newline. Settings › General
  offers "Press Return to Send", after which Shift-Return inserts a newline.
  Marked IME text is never sent.
- C4. The dark `AccentColor` fill becomes #C15634, the light hue, so white
  labels reach 4.5:1. Copper text and glyphs on dark surfaces use
  `JetDesign.accentText`, which keeps #E3A080. Increased Contrast variants are
  fill #873B23 light and #A94B2D dark, and text #873B23 light and #EDB597
  dark.
- C5. Copper stays forced at the scene roots with `.tint(JetDesign.accent)`,
  as on the colleague's branch, because the user asked to keep the visual
  gamut. This deliberately deviates from the HIG, where a person's accent
  setting replaces the app's accent.

### Delegated assumptions

These presentation choices were made on the user's behalf within C1. Each
rests on a constraint that later work may remove.

| Assumption | Reason | Consequence |
|---|---|---|
| A1. Tasks sort by creation date | Last activity is not on the wire, and rows must not jump | An older task with new activity does not rise; Needs You, unread dots and notifications carry attention |
| A2. The phase is mapped from the latest tool name | There is no structured phase | Unknown tools read "Working"; the phase must never drive behavior |
| A3. History comes from a 32 × 256 cache and a bounded replay | There is no per-Conversation history query | After a relaunch, older history may show a summary card instead |
| A4. Assistant names come from Runs this client started | Runs don't expose their Craft | Other tasks read "Assistant", and their subtitle omits the assistant |
| A5. Continuation always uses `submit_turn` with the pinned assistant | Switching loses native context | Using another assistant needs a new task |
| A6. One main window | The session holds one selection | Open in New Window waits for a per-window session |
| A7. Drafts live in memory only | Client memory never stores prompts | Quitting Jet loses unsent drafts |
| A8. The Keep sheet prefills the branch from the prefix and a title slug | The branch name appears in the outcome and the `git switch` line | Manual branches differ from automatic delivery, which names branches with the prefix and Conversation UUID |
| A9. The draft pull request plan always shows the token note | Jet cannot check the GitHub token | People with a working token still see the note |
| A10. Stop and Move to Jet Trash waits up to 30 seconds | A Run must end before the task is forgotten | A Run that takes longer to stop fails the second check, and the task is not forgotten |
| A11. Pairing polls every 2 seconds | There is no pairing-claim event | A short delay before the match check appears |

### Open items

- O1. Implementation verification of the macOS column of the capability
  audit. It is added when the redesign lands.
- O2. Whether `TextField(axis: .vertical)` inserts a newline on Return on
  device. If it does not, the composer switches to a `TextEditor` with a
  placeholder overlay.
- O3. Whether the Tauri app adopts the macOS lexicon, the Keep Changes sheet
  and the non-modal inspector. Until it is decided, the two apps use different
  words and flows for the same work.
- O4. Resolved: the unread dot and the JetMark are copper glyphs on plain
  surfaces, so they use `JetDesign.accentText`.
- O5. The protocol gaps above. Each closed gap lets the matching client
  fallback go.
- O6. The deferred macOS work above, including the copper app icon.
