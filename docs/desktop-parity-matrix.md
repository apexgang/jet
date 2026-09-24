# Desktop parity matrix

Status: seeded in Wave 3.4 (Tauri/Linux). The Linux column is verified by
automated tests and the Chromium audit; native checks are pending. The
macOS column is observed only. The platform exceptions and deferrals below
are **Proposed** until the product owner approves or rejects each one.

- Linux column verified by automated tests and the Chromium audit on branch
  `design-wave-3-tauri` at the Wave 3.4 S5 commit (parent `d328cc6`) on
  2026-09-23. The manual native checklist in `apps/jet-tauri/docs/wave-3.4.md`
  has not been run; a row whose acceptance needs one of its checks reads
  `Pending manual check N` until the result is recorded here with its date and
  desktop.
- macOS column observed read-only at `79d450a` (the last commit that touched
  `apps/jet`, unchanged at `d328cc6`). The Swift owner confirms or corrects
  it. This change does not edit `apps/jet`.
- Wave 4 rows W4-1 to W4-6 (Linux distribution) added on 2026-09-24 at
  `fb346e1` on branch `w4/docs`. Their Linux cells rest on automated tests
  against fakes; the CI journey and Homebrew check that would accept them
  have not run (`apps/jet-tauri/docs/wave-4.md`). Their macOS cells are not
  assessed.

This is the cross-client acceptance record that
`docs/desktop-implementation-plan.md` ("Cross-client acceptance") asks for.
Rows are user-visible capabilities, not components. A capability passes only
when the two clients match on five criteria, with native platform differences
allowed:

- **W** wording (the same vocabulary and copy intent);
- **C** consequence disclosure (what an action affects, before it runs);
- **R** state recovery (loading, stale, offline, failed, retry);
- **P** protocol semantics (the same Queries, Commands, revisions, cursors);
- **A** accessibility (keyboard, screen reader, contrast, motion).

## Legend

| Value | Meaning |
| --- | --- |
| `Pass` | Implemented on this platform, and W, C, R, P and A match the design language and the other client wherever the other client has the capability. |
| `Gap: <letters> <reason>` | Implemented with a difference in the named criteria. |
| `Gap (both)` | The protocol supports it, and neither client shows it. |
| `Pending <wave>` | Planned in a named wave for this platform, not built yet. |
| `Pending manual check N` | Built and covered by automated tests, but acceptance needs item N of the Wave 3.4 manual native checklist, which has not been run. Becomes `Pass` or `Gap` once the result is recorded. |
| `Pending release check` | Built and covered by automated tests against fakes, but acceptance needs a run on a real system (the desktop journey, the Homebrew check, or a manual check) that has not happened. The row's evidence names it. |
| `Exception PE-n` / `Deferral PD-n` | Covered by a platform exception or deferral in the table below. |
| `Backend: <dependency>` | The protocol or `jet-client` lacks the surface. The UI stays disabled or absent and says why. |
| `Absent (observed)` | Not present in the observed macOS build, with no plan recorded in this repository. |
| `n/a` | Does not apply to this platform. |
| `Not assessed` | This platform was not observed for the row. The platform's owner fills it in. |

Row IDs are stable. When a row changes, update its cells and keep the ID.

## Summary

110 capability rows. Linux cells: 77 `Pass`, 1 `Gap` (W3-25), 10 `Gap (both)`
under PD-1, 6 `Exception` (PE-1, PE-6, PE-7 and 3.2's launch at login),
7 `Backend`, 7 `Pending` (six manual checks: WS-2, WS-3, WS-4, WS-13, WS-17
and WS-19; and W3-7 appearance override) and 2 `n/a`. Several `Pass` cells also carry a backend note in their evidence.

macOS cells: 61 `Pass`, 22 `Pending 3.x (Swift)`, 9 `Absent (observed)`
(Linux is ahead: Deliver, notifications, deep links), 10 `Gap (both)`,
4 `Backend`, 1 `Gap` (W3-5 pane name) and 3 `n/a`. The Swift client stops at
Wave 2.2, so most Wave 3 rows are pending there.

Wave 4 rows (2026-09-24), not in the counts above: 6 rows, W4-1 to W4-6.
Linux cells: 6 `Pending release check`. macOS cells: 6 `Not assessed`.

The mapping checklist (section 5) shows that every source row and every
public `jet-client` request method maps to a row.

## 1. Capability matrix

### Window and shell

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| WS-1 | Default 1280x800, minimum 900x600 | App and window model | None (client-local) | Pass | Pass | `tauri.conf.json` 1280x800, min 900x600; Swift `defaultSize(1280,800)` (`jetApp.swift`), minimum 900x600 (`DesktopShellView.swift:31`) |
| WS-2 | Window geometry restoration | App and window model | None | Pass (scene restoration) | Pending manual check 3 | Linux `window-geometry.json` (logical size, maximized, monitor-clamped; position on X11 only, PE-5), covered by `cargo test`. Full screen is not restored (GNOME convention, spec Q5). Not yet observed on X11 or Wayland |
| WS-3 | Full screen | App and window model | None | Pass (system) | Pending manual check 5 | F11 (`toggle_main_window_fullscreen`); status "Full screen on/off"; failure copy `window.mode_unavailable`, covered by `tests/shell-session.test.ts`. Not yet observed in a native window |
| WS-4 | Multiple displays | App and window model | None | Pass (system) | Pending manual check 4 | Geometry clamped to a connected monitor's work area, covered by `cargo test`. Mixed-DPI position is approximate. Not yet observed with two monitors |
| WS-5 | Sidebar collapse | Sidebar | None | Pass (toolbar, `SidebarCommands`) | Exception PE-1 | Toolbar toggle in every destination header plus F9; no View menu (PE-1) |
| WS-6 | Work-panel collapse and compact overlay | Work panel | None | Pass (inspector) | Pass | Below 1101 px the panel is a focused overlay with scrim, `inert`, Escape and focus return; never open by default in compact. Swift: Hide/Show Work Panel ⌥⌘0 |
| WS-7 | Column resizing | Work panel | None | Pass (split view) | Pass | Keyboard-operable separators; sidebar 210-300, work panel 280-440, the Swift ranges (`DesktopShellView.swift:17,29`). A narrowed column's keys and drags never lower its requested width. Widths go through CSSOM for the production CSP; the native CSP check (11) is still pending |
| WS-8 | Last destination, tab, panel and widths restored | App and window model; Selection, continuity | None | Pass (`@SceneStorage`) | Pass | `shell-presentation.json`: UI layout only, no Jet identifiers (spec Q8 reads design-language l.88 as "no Jet content beyond the UUID"). Search and the overlay are never saved |
| WS-9 | "Reopen the last task" and its ordering with setup | App and window model; Return and recovery | None | Pass | Pass | Swift order (`DesktopShellView.swift:48-59`): layout, then setup redirect wins, then the task. Off: New task, panel hidden |
| WS-10 | Restore the last task across Planes | Return and recovery | Native selection file | Pending 3.1 (Swift) | Pass | Linux restores a Plane-qualified selection (3.1 `restoredSelection`). Swift has only the local Plane and restores its UUID (`@AppStorage`) |
| WS-11 | Settings window and last pane | Settings | None | Pass | Pass | Separate Settings window, Ctrl+, in both windows, last pane restored (3.2) |
| WS-12 | Command set and menus | Menus, commands, and input | None | Pass (`JetCommands.swift`) | Exception PE-1 | No GTK menubar. Per-command Linux path in PE-1 |
| WS-13 | Close window and quit; Runs continue | App and window model | None | Pass | Pending manual check 14 | Ctrl+W closes the window, Ctrl+Q quits, in both windows (the physical key decides on non-Latin layouts); with Ctrl+W and Ctrl+Q a pending layout change is saved first (the title-bar close button does not wait for it yet). Covered by `tests/shell-session.test.ts` and `tests/settings-keys.test.ts`; not yet observed in native windows. jetd keeps Runs going; notifications stop while the app is closed (`apps/jet-tauri/docs/wave-2.3.md`). An unsent draft is lost, as with the close button (spec Q14) |
| WS-14 | Local find | Menus, commands, and input | None | Absent (observed) | Exception PE-7 | Neither client has find-in-task. `JetCommands.swift` has no Find item |
| WS-15 | Help and diagnostics entry | Menus, commands, and input | `status`, `capabilities` | Absent (observed) | Exception PE-1 | Linux: Settings › Safety and system › Diagnostics (3.3), reachable by deep links; no Help menu |
| WS-16 | Shortcut map | Menus, commands, and input | None | Pass | Pass | Ctrl+N, Ctrl+K, Ctrl+Shift+O, Ctrl+,, F9, Ctrl+Alt+0, Ctrl+Alt+1..5, Ctrl+Enter, F11, Ctrl+W, Ctrl+Q, Escape. Listed in Settings › General › Keyboard shortcuts. No firing while an IME composes or a modal is open; no bare-letter shortcuts |
| WS-17 | GTK Emacs key theme | Menus, commands, and input | None | n/a | Pending manual check 12 | Ctrl+N, Ctrl+K and Ctrl+W are Emacs editing keys in GTK text fields. App shortcuts win in the webview by design (spec Q12); not yet recorded on a GNOME host |
| WS-18 | Appearance: system light and dark | Visual language | None | Pass | Pass | Tokenized theme, WCAG table in `tests/theme-contract.test.ts`, audited in both schemes |
| WS-19 | Keyboard-only use of critical actions | Menus, commands, and input | None | Pass (manual VoiceOver and keyboard checks per plan) | Pending manual check 10 | Spec §7.8 table. Audit Tab walk: every focus stop has a visible indicator and is not covered, in 26 configurations; focus return is covered by component tests. The keyboard-only run and Orca spot check are not yet recorded |

### Sidebar

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| SB-1 | New task | Sidebar; New task | None | Pass | Pass | Ctrl+N (⌘N) focuses an empty composer |
| SB-2 | Search | Sidebar; Selection, continuity | `search` per Plane | Pass (local Plane) | Pass | Linux queries every Plane and shows one group per Plane in its own rank order (3.1). Ctrl+K focuses the field |
| SB-3 | Needs attention | Sidebar | `run_execution`, `turn_queue`, Events | Backend: `needs_attention_query` | Backend: `needs_attention_query` | Both count only the selected task's approvals and Run attention (Linux adds the selected Plane's health notice). Neither guesses attention across tasks |
| SB-4 | Projects list and selection | Sidebar; Setup and Projects | `projects` | Pass | Pass | Selection is disposable client state |
| SB-5 | Pinned tasks | Sidebar | None | Backend: `conversation_layout` | Backend: `conversation_layout` | Absent on both: parity |
| SB-6 | Recent tasks and pagination | Sidebar; Selection, continuity | `conversations`, `next_conversations` | Pass (local Plane) | Pass | Linux merges every Plane newest first and restarts on `pagination_stale` (3.1). Backend note: `recent_conversations_query` (pages come oldest first) |
| SB-7 | Schedules | Sidebar; Settings | `scheduled_tasks`, `create_schedule`, `cancel_schedule` | Pending 3.2 (Swift): placeholder "Schedules are planned for Wave 3." | Backend: `jet_client_schedules` | Linux discloses "Scheduled tasks can't be shown in this version of Jet for Linux yet." (3.2). `docs/desktop-protocol-ui-matrix.md:45` "Supported" is wrong for `jet-client` |
| SB-8 | Planes destination | Sidebar | `status`, `capabilities`, `pairing` | Pending 3.1 (Swift): placeholder | Pass | Plane list, detail with feature table, add, pair again, forget (3.1) |
| SB-9 | Local Plane label | Vocabulary | None | Pass ("This Mac", hard-coded) | Exception PE-6 | Linux "This computer". Backend: `plane_display_name` (`PlaneStatus` has no device name, `packages/jet-protocol/src/message/mod.rs:167-195`) |

### Conversation

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| CV-1 | Open a task | Conversation; Selection, continuity | `conversation`, then `events` after its cursor | Pass | Pass | Full snapshot replaces the projection after `cursor_expired` or `cursor_ahead` |
| CV-2 | Header facts | Conversation | `conversation`, `run_execution` | Pass | Pass | Task, Project, status and "Runs on". Status labels match Swift (`DesktopShellView.swift:397-421`) |
| CV-3 | Grouped timeline | Conversation; Active run | `events` | Pass | Pass | Unknown or raw events grouped as "N background updates"; nothing executed |
| CV-4 | Offline and stale label | Selection, continuity; Return and recovery | Transport state | Pass | Pass | Cached content stays and is labeled stale |
| CV-5 | Composer byte limit | Active run | `turn_queue` limits | Pass | Pass | "Message 0 / 65,536 bytes"; over-limit uses the danger text token |
| CV-6 | Send shortcut | Menus, commands, and input | `submit_turn` | Pass (⌘↩) | Pass | Ctrl+Enter |
| CV-7 | Context row | Conversation | `projects`, `capabilities` | Pass | Pass | Project, Agent, Runs on. Wave 3.4: shown as named (no forced capitals), exposed as a named group |
| CV-8 | Create-and-start | Selection, continuity; New task | `create_conversation`, `start_run` | Pass | Pass | Both clients create, then start with the first Turn. Linux uses `create_conversation_in` with Jet's managed Workspace |
| CV-9 | Send while idle or active, retry | Selection, continuity | `submit_turn` | Pass | Pass | Stable command ID across exact-body retries |
| CV-10 | New task never posts into another task | New task | `create_conversation` | Pass | Pass | D17 fixed in 3.4: a Plane event no longer selects a task while New task, Search or a Project is open (`tests/shell-session.test.ts`) |
| CV-11 | Completion | Completion and delivery | `conversation`, `run_execution`, Events | Pass | Pass | Terminal lifecycle has no live activity |
| CV-12 | Task status for screen readers | Active run | `run_execution` | Pass ("Task status: …") | Pass | Dedicated visually hidden status region; the timeline is no longer one live region (D8) |

### Conversation actions (Gap on both clients, PD-1)

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| CA-1 | Rename task or Run | Menus, commands, and input | `set_conversation_name`, `set_run_name` | Gap (both) (menu item shown disabled) | Gap (both) | Deferral PD-1. Swift `JetCommands.swift:39-40` |
| CA-2 | Fork | Menus, commands, and input | `fork_conversation` | Gap (both) (menu item shown disabled) | Gap (both) | Deferral PD-1. Swift `JetCommands.swift:41-42` |
| CA-3 | Handoff to another Harness | Conversation | `handoff_conversation` | Gap (both) | Gap (both) | Deferral PD-1 |
| CA-4 | Archive or forget | Menus, commands, and input | Forget and Trash Commands | Pending 3.3 (Swift) | Pass | No archive Command exists; forget is Jet Trash (W3-16). Linux "Move to Trash…" in the task header |

### Run modes (Gap on both clients, PD-1)

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| RM-1 | Supervised (Visa) run | Vocabulary; New task | `start_visa_run` | Gap (both) | Gap (both) | Deferral PD-1. Also Backend: `execution_default_settings` (no default mode key) |
| RM-2 | Direct (No-Visa) run | Vocabulary; New task | `StartNoVisaRun` through `execute_command` | Gap (both) | Gap (both) | Deferral PD-1 |
| RM-3 | No-Visa remote-tool review | Active run | `remote_tool_review`, `review_remote_tool` | Gap (both) | Gap (both) | Deferral PD-1, plus Backend: `jet_client_remote_tool_review_query` (`query` is `pub(crate)`, `packages/jet-client/src/connection/mod.rs:220`) |
| RM-4 | Low-level Run creation and transition | n/a (protocol plumbing) | `create_run`, `transition_run` | n/a | n/a | Neither client exposes these; `start_run` and `control_run` cover the user paths |

### Workspace and import (Gap on both clients, PD-1)

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| WK-1 | Workspace promotion preview and promote | Completion and delivery | `preview_promotion`, `promote_workspace` | Gap (both) | Gap (both) | Deferral PD-1. Not in `docs/desktop-protocol-ui-matrix.md`; added here |
| IM-1 | List external Conversations | Core flows | `external_conversations` | Gap (both) | Gap (both) | Deferral PD-1 |
| IM-2 | Import an external Conversation | Core flows | `import_conversation` | Gap (both) | Gap (both) | Deferral PD-1 |
| IM-3 | Resume an imported Conversation | Core flows | `resume_imported_conversation` | Gap (both) | Gap (both) | Deferral PD-1 |

### Active run

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| AR-1 | Approval card and its limitation | Active run | `run_execution`, Event Presentation | Backend: `approval_decision_command` | Backend: `approval_decision_command` | Identical disclosure copy (`Conversation.svelte`, `DesktopShellView.swift:597`). Card facts: action, target, scope, consequence |
| AR-2 | Authorize retry after a review denial | Active run | `authorize_approval_retry` | Pass | Pass | Minor-gated |
| AR-3 | Interrupt Turn and Stop Run confirmations | Vocabulary; Active run | `control_run` (`interrupt_turn`, `stop_run`) | Pass | Pass | Separate labeled controls; Stop Run destructive. Linux 3.4: the card buttons open the Run tab, focus Cancel, Escape cancels and focus returns (D6) |
| AR-4 | Queue, withdraw, queue full | Active run | `turn_queue`, `withdraw_turn` | Pass | Pass | Occupancy "1 / 128", target and position, queue-full message |

### Work panel

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| WP-1 | Changes and checkpoints | Changes and checkpoints | `change_diff`, `next_change_diff` | Pass | Pass | Current, Final, Turn, Historical |
| WP-2 | Patch chunks and verification | Changes and checkpoints | `change_artifact` | Pass | Pass | Bounded chunks, size and SHA-256 verified |
| WP-3 | Binary and limit states | Changes and checkpoints | `change_diff` availability | Pass | Pass | Storage pressure, Run budget, Artifact size |
| WP-4 | Files: edit and conflict | Files, terminals, and recovery | `editable_file`, `apply_user_edit` | Pass | Pass | Exact `FileRevision`; conflict before overwrite. `project_entry` (Project browse) is used by neither client |
| WP-5 | Structured review comments | Files, terminals, and recovery | `submit_review` | Pass | Pass | One batch becomes one user Turn |
| WP-6 | Workspace terminals | Files, terminals, and recovery | `workspace_terminals`, `open_terminal`, `close_terminal`, attach/resize | Pass | Pass | Byte credit, gaps marked; detach is not close |
| WP-7 | Run details | Work panel | `run_execution` | Pass | Pass | Lifecycle, activity, Runs on, checkpoint, cursor, revision. Linux 3.4 shows the Plane label as named |
| WP-8 | Recovery actions | Files, terminals, and recovery | Restart metadata, recovery actions | Pass | Pass | Named actions: Reload File, Refresh Task, Refresh Run, Reconnect Activity |
| WP-9 | Revision conflict | Files, terminals, and recovery | `revision_conflict.safe_state` | Pass | Pass | Safe state replaces the entity before retry |
| WP-10 | State kept when collapsed | Changes and checkpoints | None | Pass | Pass | Linux never unmounts `WorkPanel` for presentation changes (`tests/overlay.component.test.ts`) |
| WP-11 | Work-panel tabs by keyboard | Menus, commands, and input | None | Pass (⌥⌘1-4) | Pass | Ctrl+Alt+1..5 and arrow keys (APG tabs, roving tabindex). Ctrl+Alt+digit can collide with some window managers (spec Q11) |

### Delivery and notifications

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| DN-1 | Deliver: branch, commit, push, draft PR | Completion and delivery | `git_deliveries`, `deliver_git`, `acknowledge_git_delivery` | Absent (observed) | Pass | Linux ahead (Wave 2.3). Backend note: Git delivery preview (current branch, HEAD, remote URL) is missing (`apps/jet-tauri/docs/wave-2.3.md`). The Swift owner closes the macOS side |
| DN-2 | Desktop notifications | Motion and feedback | Run Events (client-local) | Absent (observed) | Pass | Opt-in; per-Plane muting (3.2); no task content in notifications |

### Setup

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| SU-1 | Connect to the local Plane and its health | First launch; Setup and Projects | Handshake, `status`, `capabilities` | Pass | Pass | Connection status independent from setup sections |
| SU-2 | First-launch status | First launch | `status`, `capabilities`, `projects`, `account_bindings` | Pass | Pass | Incomplete setup opens the Project destination at launch |
| SU-3 | Add a Project | Setup and Projects | `preview_project`, `register_project` | Pass (folder picker) | Pass | Folder picker plus the disclosed expert path fallback; Ctrl+Shift+O focuses "Choose Folder…" |
| SU-4 | Remove a Project | Setup and Projects | `preview_project_removal`, `remove_project` | Pass | Pass | Modal, typed name, Move to Trash default; shortcuts suppressed while it is open (D4) |
| SU-5 | Harness binding | Setup and Projects | `bind_account` | Pass | Pass | Non-secret binding only |
| SU-6 | Skippable remote pairing | First launch | `pairing` | Pass | Pass | Visible and optional |

### Wave 3.1-3.3 capabilities

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| W3-1 | Owner pairing and paired clients | Settings (Connections) | `pairing`, `set_pairing_gate`, `open_pairing`, `confirm_pairing`, `set_paired_client_access`, `revoke_paired_client` | Pending 3.1 (Swift): "Pairing controls arrive in Wave 3." | Pass | 3.1. Backend notes: `pairing_owner_cli`, no cancel-offer Command, no scoped access |
| W3-2 | SSH remote Planes and enrollment | Settings (Connections) | `claim_pairing`, `complete_pairing`, SSH standard I/O | Pending 3.1 (Swift) | Pass | Linux reaches `claim_pairing` and `complete_pairing` through its own enrollment path (`src-tauri/src/jet/planes`), not these two `jet-client` methods |
| W3-3 | Aggregated sidebar and search | Sidebar | Per-Plane `conversations`, `search` | Pending 3.1 (Swift) | Pass | Independent cursors and failure rows per Plane |
| W3-4 | Per-Plane unsupported capabilities | States and scale | `protocol.feature_unavailable` | Pending 3.1 (Swift) | Pass | "Update Jet on {label}"; minor numbers only in Plane detail. Backend: `client_negotiated_minor_accessor` |
| W3-5 | Five Settings groups | Settings | `settings` | Gap: W "Safety & System", Work and Safety placeholders | Pass | Linux "Safety and system" (`wave-3.2.md:855-856`). The Swift owner aligns the name or records an exception |
| W3-6 | Launch at login | Settings (General) | None | Absent (observed) | Exception (3.2) | "Starting Jet at login isn't available on Linux yet." See the 3.2 exception |
| W3-7 | Appearance override | Settings (General) | None | Absent (observed) | Pending (3.2 follow-up) | 3.2 shipped system light/dark only ("Jet follows your system's light or dark setting."). Nothing to audit until it lands |
| W3-8 | Accounts | Settings (Agents) | `account_bindings`, `bind_account`, `unbind_account` | Pending 3.2 (Swift): bind only, from Setup | Pass | Linux: detail, quota windows, reviewed unbind (3.2) |
| W3-9 | Usage | Settings (Agents) | `usage`, `usage_history` | Pending 3.2 (Swift) | Pass | Totals and history table |
| W3-10 | Harnesses, Crafts and extensions | Settings (Agents) | `discover_craft`, `install_craft`, `disable_craft`, `extension_catalog`, `inspect_extension`, `change_extension`, `extension_change` | Pending 3.2 (Swift) | Pass | Backend notes: `craft_models`, `craft_enabled_state`, `craft_enable_command`, `extension_changes_list` |
| W3-11 | Execution defaults | Settings (Safety and system) | `settings` | Pending 3.2 (Swift) | Backend: `execution_default_settings` | No default Harness, Craft or Visa key exists |
| W3-12 | Reviews settings | Settings (Work) | `settings`, `set_setting`, `clear_setting` | Pending 3.2 (Swift) | Pass | Last-writer-wins disclosed (`setting_expected_revision`) |
| W3-13 | Retention settings | Settings (Work) | `settings` | Pending 3.2 (Swift) | Pass | Grace days, auto-delete rule count |
| W3-14 | Notification routing | Settings (General) | Client-local | Absent (observed) | Pass | Per-Plane muting. Sound cues: Backend `sound_cue_routing` |
| W3-15 | Error deep links to Settings | Settings | Stable error codes | Absent (observed) | Pass | Typed targets, never URLs (3.2, 3.3) |
| W3-16 | Jet Trash: forget, restore, delete everywhere | Return and recovery | `forget_conversation`, `delete_conversation_everywhere`, `restore_conversation`, `conversation_trash`, `retention_preview` | Pending 3.3 (Swift) | Pass | Concrete dates and Plane names; 256 cap disclosed. Backend notes: `native_deletion_capability`, `trash_pagination`, `retention_policy_command` |
| W3-17 | Auto-delete review | Return and recovery | `autodelete_rules`, `compile_autodelete_rule`, `set_autodelete_rule_inactive_days`, `approve_autodelete_rule`, `authorize_autodelete_everywhere`, `delete_autodelete_rule` | Pending 3.3 (Swift) | Pass | Model output never becomes a Command |
| W3-18 | Recovery snapshots | Return and recovery | `status`, `restore_recovery_snapshot`, `purge_recovery_snapshots` | Pending 3.3 (Swift) | Pass | Backend notes: `jet_client_orphaned_executions`, `recovery_bundle_transport` |
| W3-19 | Disk pressure | States and scale | Admission refusals | Pending 3.3 (Swift) | Backend: `disk_pressure_status` | Shown as "last refused at {time}", never a gauge |
| W3-20 | Security audit and diagnostics | Security and privacy behavior | `security_audit_after`, `begin_audit_epoch`, `status`, `capabilities` | Pending 3.3 (Swift) | Pass | Redacted by default; evidence export. Backend: `diagnostic_log_query` |
| W3-21 | Service health, versions and repair | Settings (Safety and system) | `status`, `capabilities` | Pending 3.3 (Swift) | Pass | One health notice in the main window, no metrics dashboard |
| W3-22 | Plane transfer | Return and recovery | None | Backend: `plane_transfer_gui` | Backend: `plane_transfer_gui` | Absent on both: parity. Transfer tombstones in Trash are shown read-only on Linux |
| W3-23 | Auto-continue | Settings (Agents) | `auto_continue`, `set_auto_continue` | Pending 3.2 (Swift) | Pass | Reviewed change |
| W3-24 | Utility Provider | Settings (Agents) | `utility`, `settings` | Pending 3.2 (Swift) | Pass | Utility results grant no authority |
| W3-25 | Creation-time retention choice | New task | `create_conversation` retention | Absent (observed) | Gap: C always Retain | Linux sends `Retain`; the creation-time choice is client work deferred from 3.3 (`apps/jet-tauri/docs/wave-3.3.md`) |

### Wave 4 distribution (Linux)

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| W4-1 | Automatic core installation | First launch; Setup and Projects | None: native `jetd core status`, `stage` and `activate`, then `systemctl --user` or XDG autostart | Not assessed | Pending release check | Decision table in `local_service/decision.rs`. Setup shows each phase, then Repair or Check again. `local_service/tests.rs` and the Setup tests run against a simulated `jetd core`, systemd and brew. The desktop journey checks a first install from the `.deb` and has not run |
| W4-2 | Core channel and versions | Settings (Safety and system) | `jetd core status`; `status` | Not assessed | Pending release check | "Managed by this app", "Managed by Homebrew" or "Development build", with the current, running, previous and bundled versions (`versions-service.component.test.ts`). The journey checks "Managed by this app" and has not run |
| W4-3 | Core rollback | Settings (Safety and system) | None: `jetd core rollback` behind a one-use native review | Not assessed | Pending release check | The review names both versions and says running tasks keep running and the newer version stays installed. Stale and expired reviews are refused. Fakes only; no real rollback has run |
| W4-4 | App updates | Settings (Safety and system) | None: Tauri updater and the signed `latest.json` on GitHub | Not assessed | Pending release check | Off with the reason shown for Homebrew, development builds and unsupported bundles. Automatic-check preference with a note about github.com; restart confirmation. Fakes only; the journey checks only which controls show |
| W4-5 | Homebrew install | Platform adaptation | None: the `apexgang/tap/jet` formula and the Linux `jet-app` cask | Not assessed | Pending release check | `brew install apexgang/tap/jet apexgang/tap/jet-app`. The app starts the formula's service and turns its updater off. `homebrew-check.yml` has not run |
| W4-6 | Launcher entry | App and window model; Platform adaptation | None (client-local) | Not assessed | Pending release check | deb and rpm install their own entry. An AppImage writes `me.heeka.jet-tauri.desktop` with `TryExec`, and its icon, at launch (`launcher.rs` tests). No bundle has been launched to check it |

### Error and recovery states

| ID | State | Design ref | Protocol evidence | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| ER-1 | Offline | States and scale | Transport loss, `unavailable` | Pass | Pass | Last trustworthy state kept and labeled; Commands disabled |
| ER-2 | Stale Event cursor | Return and recovery | `cursor_expired`, `cursor_ahead` | Pass | Pass | Fresh snapshot, then resume; "Jet refreshed the full Conversation snapshot." |
| ER-3 | Stale page | Selection, continuity | `pagination_stale` | Pass | Pass | Restart from the first page |
| ER-4 | Denied | States and scale | `unauthorized` | Pass | Pass | Named scope; Pair again where it applies (3.1) |
| ER-5 | Revision conflict | Files, terminals, and recovery | `revision_conflict.safe_state` | Pass | Pass | See WP-9 |
| ER-6 | Unsupported | States and scale | Minor refusal, `incompatible`, missing tool | Pass | Pass | Rest of the product stays available |
| ER-7 | Outcome unknown | Completion and delivery | `outcome_unknown` | Pass | Pass | Acknowledge; never replay (Linux Deliver) |
| ER-8 | Rate limited | States and scale | `rate_limited` | Pass | Pass | Draft preserved |
| ER-9 | Internal failure | Security and privacy behavior | `internal` | Pass | Pass | Safe retry only when `retryable` |

### iOS

| ID | Capability | Design ref | Protocol path | macOS | Linux | Notes and evidence |
| --- | --- | --- | --- | --- | --- | --- |
| IOS-1 | iOS target and remote companion | App and window model | n/a | Not a desktop platform; owned by the Swift plan | n/a | Kept buildable by the Swift plan; out of this matrix |

## 2. Adaptation configurations

Linux methods: **Vitest** (node: layout, shortcut, session and theme
contract, with WCAG contrast computed per scheme), **component** (happy-dom
and `@testing-library/svelte`), **audit** (`just adaptation-audit`: Chromium
through `agent-browser` against mocked IPC; it proves CSS, layout logic and
axe rules, not WebKitGTK, the CSP or native windows), **manual** (the native
checklist in `apps/jet-tauri/docs/wave-3.4.md`).

| Configuration | Linux method | Linux result (2026-09-23) | macOS (observed) |
| --- | --- | --- | --- |
| Narrow 900x600 | Vitest, component, audit | Pass: 8 scenes × 8 configurations, no overflow, overlay closed after load, Send not covered | Pending Swift QA |
| Compact edge 1100 | Vitest, audit | Pass: compact overlay model | Pending Swift QA |
| Regular edge 1101 | Vitest, audit | Pass: conversation keeps 420 px | Pending Swift QA |
| Default 1280x800 | Audit | Pass (light, dark and every media feature below) | Pending Swift QA |
| Wide 1920 | Audit | Pass | Pending Swift QA |
| Wide 2560 | Vitest, audit | Pass: the conversation grows; columns stay in range | Pending Swift QA |
| HiDPI 2x | Audit | Pass at 1280x800@2 | Pending Swift QA |
| Full screen | Manual | Pending (checklist 5) | System behavior |
| Multiple displays | Manual | Pending (checklist 4) | System behavior |
| Dark | Vitest, audit | Pass | Pending Swift QA |
| Light | Vitest, audit | Pass after the Wave 3.4 fixes (see the wave doc) | Pending Swift QA |
| Appearance override | n/a | 3.2 has no override yet (W3-7) | Absent (observed) |
| Increased contrast | Vitest, audit | Pass in Chromium (light and dark). Whether WebKitGTK maps a desktop high-contrast setting is manual check 7 (possible PE-4) | Pending Swift QA |
| Forced colors | Vitest, audit | Pass: every disabled, focus and status rule has a system-colour counterpart | n/a (no forced colours on macOS) |
| Reduced motion | Vitest, audit | Pass: no running animation after 1 s | Pending Swift QA |
| Reduced transparency | Vitest, audit | Pass in Chromium (nothing is translucent). WebKitGTK 2.52.6 does not expose the media feature (PE-2) | Pending Swift QA |
| Large text | Manual | Pending (checklist 9) | Pending Swift QA |
| Keyboard only | Component, audit, manual | Pass in the audit Tab walk; manual pass pending (checklist 10) | Pending Swift QA |
| Emacs key theme | Manual | Pending (checklist 12) | n/a |
| Screen reader | Component, manual | Roles and names covered by component tests and axe; Orca pending (checklist 10) | VoiceOver per plan |

## 3. Platform exceptions and deferrals

Every entry needs the product owner's decision before Wave 3.4 closes for
Linux. None has been approved yet.

| ID | Design-language requirement | Linux behaviour | Rationale | Approval |
| --- | --- | --- | --- | --- |
| PE-1 | Every toolbar command also in a menu bar; complete command set (Menus, commands, and input) | No GTK menubar. New task: sidebar button and Ctrl+N. Add or open Project: Setup and Ctrl+Shift+O. Close window: Ctrl+W. Quit: Ctrl+Q. Standard editing: WebKitGTK text fields (context menu, standard keys). Local find: none (PE-7). Global Search: sidebar and Ctrl+K. Send: button and Ctrl+Enter. Interrupt Turn and Stop Run: card and Run-tab buttons with confirmation. Rename, Fork, Archive: absent (PD-1; forget is Jet Trash). Sidebar: header button and F9. Work panel: header button and Ctrl+Alt+0. Tabs: tabs and Ctrl+Alt+1..5. Full screen: F11. Help and diagnostics: Settings › Safety and system › Diagnostics, no Help menu | A menubar needs a native-to-webview command channel and a second command surface to keep in sync | Proposed, product owner, pending |
| PE-2 | Respect Reduce Transparency (Motion and feedback) | The `prefers-reduced-transparency` block exists, but WebKitGTK 2.52.6 does not expose the feature (the string is absent from `libwebkit2gtk-4.1.so`). Nothing in the UI is translucent, so the requirement holds by construction | Engine limitation | Proposed, product owner, pending |
| PE-4 (conditional) | Increased contrast (Visual language) | Only if manual check 7 shows WebKitGTK does not report `prefers-contrast: more` for the desktop's high-contrast setting. Follow-up would be a native read exposed as `data-contrast` | Engine limitation, not yet observed | Not triggered yet; decide after check 7 |
| PE-5 | Restorable window on multiple displays (App and window model) | Window position is restored on X11 only. Wayland does not let a client place its window; size and maximized state are restored | Wayland protocol | Proposed, product owner, pending |
| PE-6 | Plane shown as "This Mac or the device name" (Vocabulary) | The local Plane is "This computer"; a remote Plane is its SSH address. Backend: `plane_display_name` | No device name in the protocol; "This Mac" is wrong on Linux | Proposed, product owner, pending |
| PE-7 | Edit menu: local find (Menus, commands, and input) | No find in the task timeline. Search (Ctrl+K) finds tasks; WebKitGTK has no page-find UI. macOS has no local find either (WS-14) | Needs a find UI for a virtualized timeline; no Linux menu to host it | Proposed, product owner, pending |
| 3.2 exception | Launch at login (Settings, General) | Not available; General says so | No autostart plugin; dependency review needed (`apps/jet-tauri/docs/wave-3.2.md`) | Recorded by 3.2; approval tracked there |
| PD-1 | Rename, fork, handoff, promotion, import, supervised and direct runs (Menus, commands, and input; Vocabulary) | Absent on both clients (CA-1..3, RM-1..3, WK-1, IM-1..3) | Each needs its own preview and confirmation, revision-conflict handling and copy; beyond adaptation work. Proposed for a dedicated conversation-actions wave on both clients | Proposed, product owner, pending |

PE-3 is retired: Settings is a separate window (3.2).

## 4. Design-language coverage

Every heading of `docs/design-language.md` and the rows that carry it.

| Heading | Rows |
| --- | --- |
| Product intent | CV-1, CV-4, SU-1, WS-9 |
| Product principles | SB-1, CV-2, CV-3, WS-9, WS-12, WS-19 |
| Vocabulary | SB-9 (PE-6), AR-3, RM-1, RM-2 |
| App and window model | WS-1, WS-2, WS-3, WS-4, WS-9, WS-11, WS-13, IOS-1, W4-6 |
| Main workspace | WS-5, WS-6, WS-7 |
| Sidebar | SB-1 to SB-9, WS-5 |
| Conversation | CV-1 to CV-12 |
| Selection, continuity, and activity | SB-2, SB-6, CV-1, CV-4, CV-8, CV-9, WS-8, WS-10, ER-2, ER-3 |
| Work panel | WS-6, WS-7, WP-7, WP-10, WP-11 |
| Changes and checkpoints | WP-1, WP-2, WP-3, WP-10 |
| Files, terminals, and recovery | WP-4, WP-5, WP-6, WP-8, WP-9, ER-5 |
| Work-panel states | WP-1 to WP-8 |
| Core flows | SU-1 to SU-6, CV-8, AR-1 to AR-4, DN-1, W3-16, IM-1 to IM-3 |
| First launch | SU-1, SU-2, SU-6, W4-1 |
| Setup and Projects | SU-1, SU-3, SU-4, SU-5 |
| New task | SB-1, CV-8, CV-10, RM-1, RM-2, W3-25 |
| Active run | AR-1, AR-2, AR-3, AR-4, CV-3, CV-5, CV-12 |
| Completion and delivery | CV-11, DN-1, WK-1, ER-7 |
| Return and recovery | WS-9, WS-10, ER-2, W3-16, W3-17, W3-18, W3-22 |
| States and scale | ER-1 to ER-9, W3-4, W3-19 |
| Visual language | WS-18, W3-7; section 2 (dark, light, contrast, forced colours) |
| Motion and feedback | DN-2; section 2 (reduced motion, reduced transparency, PE-2) |
| Menus, commands, and input | WS-12, WS-14, WS-15, WS-16, WS-17, WS-19, CV-6, WP-11, CA-1 to CA-4 |
| Settings | WS-11, W3-5 to W3-15, W3-21, W3-23, W3-24, W4-2 to W4-4 |
| Security and privacy behavior | W3-20, ER-9, WS-8 (no Jet content in local layout state) |
| Platform adaptation | This whole matrix; section 3; W4-5, W4-6 |
| Evidence ledger | n/a (sources, not behaviour) |
| Confirmed decisions and open dependencies | Section 3 approvals; Backend cells |

## 5. Mapping checklist

Every source row and public `jet-client` request method, and the matrix row
that covers it. None is unmapped.

### `docs/desktop-protocol-ui-matrix.md` rows

| Source row | Matrix row |
| --- | --- |
| Connect to a Plane | SU-1 |
| First-launch status | SU-2 |
| List and select Projects | SB-4 |
| Add a Project | SU-3 |
| Remove a Project | SU-4 |
| Recent Conversations | SB-6 |
| Search | SB-2 |
| Needs attention | SB-3 |
| Pinned Conversations | SB-5 |
| Schedules | SB-7 |
| Plane and pairing management | SB-8, W3-1, W3-2 |
| Open a Conversation | CV-1 |
| New task | CV-8, RM-1, RM-2 |
| Timeline | CV-3 |
| Send while idle or active | CV-9 |
| Withdraw queued input | AR-4 |
| Harness approval | AR-1 |
| No-Visa remote-tool approval | RM-3 |
| Retry an automatic-review denial | AR-2 |
| Interrupt current Turn | AR-3 |
| Stop Run | AR-3 |
| Rename Conversation or Run | CA-1 |
| Fork | CA-2 |
| Handoff to another Harness | CA-3 |
| Completion | CV-11 |
| Queue | AR-4 |
| Run details | WP-7 |
| Changed files and diff | WP-1 |
| Patch Artifact chunks | WP-2 |
| Files | WP-4 |
| Structured review comments | WP-5 |
| Workspace terminals | WP-6 |
| Commit, push, or draft pull request | DN-1 |
| Completion notification | DN-2 |
| Jet settings | W3-12, W3-13, W3-11 |
| App appearance, window restoration, local notification preference | WS-2, WS-8, WS-18, W3-7, W3-14 |
| Harnesses and accounts | W3-8, W3-10 |
| Trash and retention | W3-16, W3-17 |
| Recovery | W3-18 |
| Security and diagnostics | W3-20, W3-21 |
| Plane transfer | W3-22 |
| Offline | ER-1 |
| Stale Event cursor | ER-2 |
| Stale page | ER-3 |
| Denied | ER-4 |
| Revision conflict | ER-5 |
| Unsupported | ER-6 |
| Outcome unknown | ER-7 |
| Rate limited | ER-8 |
| Internal failure | ER-9 |
| Fixture `first-launch` | SU-2 |
| Fixture `ready-new-task` | CV-8 |
| Fixture `active-run` | CV-1, AR-3 |
| Fixture `queued-turns` | AR-4 |
| Fixture `approval-needed` | AR-1 |
| Fixture `completed-run` | CV-11, WP-1, DN-1 |
| Fixture `offline-cached-conversation` | CV-4, ER-1 |
| Fixture `expired-event-cursor` | ER-2 |
| Fixture `approval-denied` | AR-2, ER-4 |
| Fixture `unsupported-git-delivery` | DN-1, ER-6 |
| Fixture `lost-run-recovery` | WP-7, W3-18 |

### `packages/jet-client/src/requests` public methods

| Method (file) | Matrix row |
| --- | --- |
| `begin_audit_epoch` (audit.rs) | W3-20 |
| `security_audit_after` (audit.rs) | W3-20 |
| `auto_continue`, `set_auto_continue` (auto_continue.rs) | W3-23 |
| `compile_autodelete_rule`, `set_autodelete_rule_inactive_days`, `approve_autodelete_rule`, `authorize_autodelete_everywhere`, `delete_autodelete_rule`, `autodelete_rules` (autodelete.rs) | W3-17 |
| `change_diff`, `next_change_diff` (checkpoint.rs) | WP-1 |
| `change_artifact` (checkpoint.rs) | WP-2 |
| `account_bindings`, `bind_account`, `unbind_account` (account.rs) | SU-5, W3-8 |
| `conversations`, `next_conversations` (conversation.rs) | SB-6 |
| `conversation` (conversation.rs) | CV-1 |
| `create_conversation`, `create_conversation_in` (conversation.rs) | CV-8, W3-25 |
| `set_conversation_name`, `set_run_name` (name.rs) | CA-1 |
| `status` (mod.rs) | SU-1 |
| `events_after` (mod.rs) | CV-1, CV-3 |
| `capabilities` (mod.rs) | SU-1, W3-21 |
| `preview_project`, `register_project` (project.rs) | SU-3 |
| `project_entry` (project.rs) | WP-4 |
| `projects` (project.rs) | SB-4 |
| `preview_project_removal`, `remove_project` (project.rs) | SU-4 |
| `preview_promotion`, `promote_workspace` (promotion.rs) | WK-1 |
| `disable_craft`, `discover_craft`, `install_craft` (craft_installation.rs) | W3-10 |
| `external_conversations` (import.rs) | IM-1 |
| `import_conversation` (import.rs) | IM-2 |
| `resume_imported_conversation` (import.rs) | IM-3 |
| `forget_conversation`, `delete_conversation_everywhere`, `restore_conversation`, `conversation_trash`, `retention_preview` (retention.rs) | W3-16 |
| `editable_file`, `apply_user_edit` (user_input.rs) | WP-4 |
| `submit_review` (user_input.rs) | WP-5 |
| `usage`, `usage_history` (usage.rs) | W3-9 |
| `utility` (utility.rs) | W3-24 |
| `start_visa_run` (visa.rs) | RM-1 |
| `extension_catalog`, `inspect_extension`, `change_extension`, `extension_change` (extension.rs) | W3-10 |
| `submit_turn` (turn.rs) | CV-9 |
| `turn_queue`, `withdraw_turn` (turn.rs) | AR-4 |
| `authorize_approval_retry` (review.rs) | AR-2 |
| `fork_conversation` (fork.rs) | CA-2 |
| `workspace_terminals`, `open_terminal`, `close_terminal` (terminal.rs) | WP-6 |
| `run_execution` (run.rs) | WP-7 |
| `control_run` (run.rs) | AR-3 |
| `start_run` (run.rs) | CV-8 |
| `create_run`, `transition_run` (run.rs) | RM-4 |
| `git_deliveries` (git_delivery.rs) | DN-1 |
| `handoff_conversation` (handoff.rs) | CA-3 |
| `set_paired_client_access`, `revoke_paired_client` (pairing/paired_client.rs) | W3-1 |
| `search` (search.rs) | SB-2 |
| `pairing`, `set_pairing_gate`, `open_pairing`, `confirm_pairing` (pairing/mod.rs) | W3-1, SU-6 |
| `claim_pairing`, `complete_pairing` (pairing/mod.rs) | W3-2 |
| `restore_recovery_snapshot`, `purge_recovery_snapshots` (store_recovery.rs) | W3-18 |
| `settings`, `set_setting`, `clear_setting` (setting.rs) | W3-12, W3-13, W3-11 |

`scheduled_tasks`, `create_schedule` and `cancel_schedule` have no
`jet-client` method (SB-7, Backend `jet_client_schedules`).
