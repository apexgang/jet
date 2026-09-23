# Wave 3.2: Settings and automation (Tauri / Linux)

Settings is now a separate window with five panes: General, Agents, Work,
Connections, and Safety and system. It edits Plane settings, Account
bindings, Auto-continue, Crafts and Harness extensions through reviewed
changes. It never shows task content. The main window links to the section
that can fix an error. Notifications can be muted per Plane.

## Delivered

### A separate Settings window

The Settings window has the label `settings` and is titled "Jet Settings". It is
820×620 and cannot be made smaller than 640×480. The shell creates it. It
opens from the sidebar's Settings entry, from Ctrl+, in either window, and
from deep links. Opening it again focuses and unminimizes the existing
window. Ctrl+W closes it. The last pane and section are kept in
`settings-window.json` in the app data directory. The file is written
atomically with mode 0600 and never stores a Plane.

The window has its own capability, `settings-window`. The main window
renders agent output, diffs and terminal text. It is the webview most
exposed to injected content, so it has no way to change an account, a Craft,
Developer Mode or an extension. The Settings window gets no Plane feed and
no agent content. Its change watcher forwards event kinds and Setting
identifiers only.

| Capability | Window | Commands |
|---|---|---|
| `main-plane-setup` | `main` | Wave 1–3.1 commands (Plane feeds, Planes and pairing, setup, tasks, runs, work panel, terminals, delivery), `dialog:allow-open` for the Project picker, plus `open_settings` and `load_desktop_preferences`. |
| `settings-window` | `settings` | `watch_settings_navigation`, `remember_settings_pane`, `close_settings`, `load_desktop_preferences`, `set_desktop_preferences`, `load_notification_settings`, `set_notification_settings`, `load_settings`, `prepare_setting_change`, `apply_settings_change`, `load_work_context`, `watch_settings_changes`, `load_agents`, `prepare_account_bind`, `load_account_detail`, `load_usage_history`, `prepare_auto_continue`, `prepare_account_unbind`, `prepare_craft_disable`, `pick_local_craft_source`, `discover_craft`, `load_extension_catalog`, `inspect_extension`, `prepare_extension_change`, `load_extension_change`, and the read-only `list_planes` and `load_plane_detail`. |

Only `load_desktop_preferences`, `list_planes` and `load_plane_detail` are
granted to both windows. The Settings window has no `open_plane_feed`,
`bind_harness_account`, pairing or Plane mutation, `dialog:*`, `core:*` or
plugin permission. The shell opens the local Craft file dialog itself, as a
child of the Settings window. No 3.1 grant moved: Plane, pairing and
paired-client management stay in the main window's Planes destination.

### Panes and sections

| Pane | Sections |
|---|---|
| General | Appearance ("Jet follows your system's light or dark setting."), Notifications, Restoration ("Reopen the last task when Jet starts"), and a launch-at-login notice |
| Agents | Harnesses (installed Crafts titled by Harness, one-way Disable, Add a Craft…), Extensions, Accounts (bind, detail, quota windows, Auto-continue, unbind), Usage (totals and a history table), Utility |
| Work | Projects (Project-scope rows), Delivery, Reviews, Schedules (a disclosed dependency), Retention (with the Autodelete rule count) |
| Connections | Read-only summary of the local service and the Plane list. Management stays in the main window's Planes destination. |
| Safety and system | Execution, Permissions (Developer Mode), Storage, Audit (state only) |

The Agents, Work and Safety panes are about one Plane, chosen with a Plane
picker. The Settings window never reads the main window's fixture scenario.
There is no Workspaces section, because no Workspace setting key exists.

### Placement comes from a static scope table

A Plane settings snapshot contains every key the negotiated minor names,
whatever the addressed scope. The core resolves `SettingSelection::All` with
no scope filter (`packages/jet-core/src/query/execute.rs:379-398`), and the
daemon filters rows only by minor
(`packages/jet-daemon/src/translate/setting.rs:16-21`). Scope is checked only
on writes. The snapshot therefore cannot say which scope may store a key.
`settings.rs::SETTING_SCOPES` mirrors `packages/jet-core/src/setting/key.rs`
for all 24 keys, and the TypeScript `SETTING_PLACEMENT` mirrors it. A Rust
test and a Vitest case pin both.

| Keys | Plane | Project | Conversation |
|---|---|---|---|
| `storage.disposable_mib`, `git.message_instructions`, `retention.trash_grace_days`, `security.audit_retention_days`, `utility.git_text`, `utility.content_consent`, `utility.account_binding`, `utility.autodelete_compilation`, `craft.developer_mode`, `review.automatic`, `review.account_binding`, `review.cross_provider_consent`, `artifact.max_mib`, `artifact.run_mib`, `energy.concurrency`, `energy.low_power_concurrency`, `energy.constrained`, `energy.foreground_override` | yes | – | – |
| `git.auto_commit`, `git.auto_branch`, `git.auto_push`, `git.auto_draft_pull_request`, `git.branch_prefix` | – | yes | yes |
| `utility.automatic_naming` | yes | yes | yes |

A key at a scope the table forbids is refused with `settings.scope_invalid`
before any I/O. The daemon's `setting.scope_unsupported` stays the authority.
A Plane pane never renders the built-in value of a Project-only key. When a
key is missing from the snapshot, the Plane's minor predates it, and the row
reads "Needs a newer Jet service on this Plane." Conversation-scope overrides
are out of scope, and `load_settings` refuses Conversation scope.

### Deep links

`open_settings(target)` takes a typed `{pane, section, plane_id}`. When the
window is closed, the target is held and delivered once on the navigation
channel after the window opens. `settingsTargetForError` matches an exact code
first, then a prefix. It returns a target only for a section that has landed.
A Plane pane's link carries the Plane.

| Codes | Target |
|---|---|
| `notifications.permission_denied` | General › Notifications |
| `transport.offline`, `protocol.incompatible`, `protocol.feature_unavailable`, `protocol.unsupported_minor` | Connections › Local service (Connections › Planes for a remote Plane) |
| `git.invalid_policy`, `git.policy_changed` | Work › Delivery |
| `retention.grace_unreadable` | Work › Retention |
| `energy.budget_exhausted`, `energy.policy_unreadable` | Safety › Execution |
| `storage.disk_pressure` | Safety › Storage |
| `craft.developer_mode_required` | Safety › Permissions |
| `security.audit_degraded`, `review.audit_degraded` | Safety › Audit |
| `account.not_found`, `account.provider_unsupported`, `account.provider_unavailable`, `auto_continue.invalid_policy`, `review.credential_unavailable`, `utility.credential_unavailable` | Agents › Accounts |
| `craft.disabled`, `craft.revoked`, `craft.update_pending`, `craft.installation_stale`, `craft.installation_failed` | Agents › Harnesses |
| `utility.consent_required`, `utility.disabled`, `utility.binding_unavailable` | Agents › Utility |
| `review.consent_required`, `review.binding_unavailable` | Work › Reviews |
| `extension.*` | Agents › Extensions |

`recovery.read_only` has no target until 3.3 adds a Recovery section. Main
window links: the Plane-unavailable notice in a conversation, SetupPanel's
accounts issue and credential-store warning, the conversation status "Sign-in
needed" and "Quota paused" (Agents › Accounts), and the Project removal
obstacle "Disable scheduled tasks first" (the Schedules destination). The
approval-card link to Work › Reviews is not implemented, because an approval
presentation carries no error code to map.

### Snapshots, fresh-read guards and the ledger

- `load_settings` returns a snapshot fenced by the Plane cursor and an opaque
  snapshot ID bound to that Plane (32 snapshots, 30-minute lifetime).
- `prepare_setting_change` reads the key again. If the value differs from the
  value the user saw, the result is `changed` and no Command is sent.
  Otherwise it returns a review. The UI asks the user to confirm the review
  for sensitive keys and applies other keys right away.
- `apply_settings_change` reads the key again on its first attempt only. The
  review ID is the Command ID.
- After an uncertain outcome (transport loss, timeout), a retry resends the
  same Command ID and byte-equal body without a new guard read. The daemon
  deduplicates it (ADR-0093).
- A definite refusal ends the review, and the next attempt needs a new review.
- Snapshots, reviews and grants are kept per Plane and are refused through
  another Plane's handle. They live in memory only and are not kept across a
  relaunch. The ledger holds at most 128 reviews for 10 minutes, and evicts
  only reviews that were never attempted.
- Writes stay last-writer-wins, because `SetSetting` has no expected revision.
  Every Plane pane has this footer: "Jet applies the most recent change. If
  another device changes the same setting at the same time, the later change
  wins."

Account bind, Auto-continue, unbind, Craft disable, Craft install and
extension changes use the same ledger. Binding is Harness-native only. The
change watcher (`watch_settings_changes`) polls the Plane's journal and
forwards only 13 staleness kinds (`setting.*`, `account.*`, `auto_continue.*`,
`project.*`, `usage.recorded`, `audit.epoch_begun`, `schedule.*`) with the
Setting key and scope. A section becomes "Changed on <Plane> since you opened
this page." only for events past its own cursor, so the user's own change does
not trigger it. Harnesses and Extensions have no events. They reload on window
focus and on Check again, and a queued extension change is polled every
2 seconds while the window is visible.

### Plane Security and Recovery banners

`load_settings`, `load_agents` and `load_work_context` read
`PlaneStatus.security` and `.recovery` in the same connection as their other
reads. A read-only Recovery Plane shows "This Plane is in read-only recovery.
You can look at its settings, but changes wait until it's restored." Every
mutation control on it is disabled. A degraded audit shows "Changes to trust
and policy are paused until this Plane's audit is repaired (Safety › Audit)."
The client does not copy the daemon's list of guarded changes, so ordinary rows
stay editable. A `security.audit_degraded` refusal is shown on the row and
reloads the banner.

### Per-Plane notification routing

Notification preferences are now version 2:
`{enabled, approvals, completion, failure, mutedPlanes}` in
`notification-preferences-v2.json`. `mutedPlanes` holds up to 64 Plane handles
(`local` or a registry UUID). An empty list means every Plane can notify.

- On first load without a v2 file, the v1 file
  (`notification-preferences.json`) is read with `mutedPlanes = []`. The v1
  file is left in place and not read again once v2 exists. An unreadable v2
  file means the defaults (notifications off), never the older v1 values.
- Handles no longer in the Plane registry are dropped when the file is loaded
  and when preferences are saved. Forget drops the Plane's fence and routing
  in memory.
- A muted Plane's events still advance its notification fence. Turning the
  Plane back on never replays what happened while it was muted.
- `load_notification_settings` also returns `planes: [{planeId, label}]` from
  the registry, without connecting. General › Notifications shows the list
  ("Notify me about") when more than one Plane is registered.
- Input is exact: an unknown field, a v1-shaped body, a malformed handle or
  more than 64 entries is refused with `preferences.invalid`.
- The file is written owner-only (0600), fsynced and atomically replaced on
  `spawn_blocking`, outside the notification gate lock. Saves are serialized
  so memory and disk agree. A failed write changes nothing.

### Restoration preference

`desktop-preferences.json` holds `{reopenLastTask}` (default on). When it is
off, launch restores no task. The selection file is still kept current.

## Remaining backend dependencies

- **`jet_client_schedules`.** `jet-client` has no `scheduled_tasks`,
  `create_schedule` or `cancel_schedule` method (none in
  `packages/jet-client/src/requests/*.rs`), and `Client::query` is
  `pub(crate)` (`packages/jet-client/src/connection/mod.rs:220`). Schedules
  shows "Scheduled tasks can't be shown in this version of Jet for Linux yet."
  with no list and no create button.
- **`plane_scheduled_tasks`.** `QueryRequest::ScheduledTasks` is per
  Conversation (`packages/jet-protocol/src/message/query.rs:76-80`). There is
  no Plane-wide read.
- **Incorrect parity row.** `docs/desktop-protocol-ui-matrix.md:45` lists
  Schedules as "Supported for Wave 3". That is wrong for `jet-client` until
  `jet_client_schedules` lands.
- **`setting_expected_revision`.** `CommandRequest::SetSetting` has no expected
  revision (`packages/jet-protocol/src/conversation/command.rs:233-248`), so
  writes are last-writer-wins. The UI says so.
- **`execution_default_settings`.** No `SettingKey` exists for a default
  Harness or Craft, or for Visa/No-Visa mode
  (`packages/jet-protocol/src/setting.rs:29-109`). No local default is stored.
- **`craft_models`.** No Query lists Models, and `InstalledCraft` has none
  (`packages/jet-protocol/src/capability.rs:134-144`). The UI says "Models are
  chosen by each Harness."
- **`craft_enabled_state`.** `InstalledCraft` has no enabled flag
  (`capability.rs:134-144`).
- **`craft_enable_command`.** There is no `EnableCraft` Command, and a disable
  persists across restarts and reinstalls (`docs/craft-lifecycle.md:22-24`).
  Disable is behind a Cancel-focused dialog that says it can't be undone from
  this app.
- **`extension_changes_list`.** Only `ExtensionChange{change_id}` exists
  (`packages/jet-protocol/src/message/query.rs:55-58`). Queued changes are
  tracked in memory by this app (64 per Plane) and are lost on relaunch.
- **`sound_cue_routing`.** ADR-0027
  (`docs/adr/0027-route-device-local-sound-cues.md`) has no wire surface: no
  `sound` or `cue` symbol in `packages/jet-protocol/src` or
  `packages/jet-client/src`. General › Notifications says "Jet for Linux
  doesn't play sound cues yet."
- **`jet_client_negotiated_minor`.** The negotiated minor is private
  (`packages/jet-client/src/connection/mod.rs:215`). The shell learns support
  from `protocol.feature_unavailable`, `protocol.unsupported_minor` and missing
  snapshot keys. A public accessor would allow a Versions row in 3.3.
- **Misleading `SettingSelection` doc comments.**
  `packages/jet-protocol/src/setting.rs:129-131` says `All` returns "every
  Setting the addressed scope may store" and `Key` is "refused when the
  addressed scope may not store it". The core does neither (see "Placement
  comes from a static scope table").

## Tauri follow-ups

- **`extension_explicit_source_picker`:** installing or updating a standalone
  extension from an explicit `kind:name@/absolute/path` source
  (`docs/harness-extensions.md:21-27`). The Extensions section says "Adding an
  extension from a folder isn't available in this app yet."
- A forced light or dark appearance. Settings follows the system for now.
- Navigation from Settings back to the main window's Planes destination. The
  Connections pane says where management lives but cannot open it.
- The approval-card deep link to Work › Reviews, once an unavailable approval
  carries an error code.
- Recorded `setup.rs` defects, not fixed here. The setup summary fabricates
  `gate: "closed", paired_clients: 0` when the pairing query fails. Account
  command IDs in setup are keyed by provider only and are not cleared after a
  definite refusal. Settings does not use that path.

## Platform exceptions

- **Launch at login.** There is no autostart plugin, and adding
  `tauri-plugin-autostart` needs a dependency review. General says "Starting
  Jet at login isn't available on Linux yet."
- **`PlatformStore` accounts.** These need a secret written from the
  presentation layer, which design-language forbids
  (`docs/design-language.md:132`). Only Harness-native binding is offered.
- **`ExternalHelper` accounts.** The helper name selects a program that
  `jetd` runs (`docs/utility-work.md:95-97`), so a webview-chosen helper would
  give the webview authority to run a program.
- **`SessionOnly` accounts.** No client path supplies the credential, and a
  daemon restart invalidates it (`packages/jet-protocol/src/account.rs:30-31`).
- **No Linux application menu.** Settings opens from the sidebar and Ctrl+,.

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

The manifest consistency test in `src-tauri/src/jet/mod.rs` checks that the
`generate_handler!` names in `lib.rs`, the `build.rs` list and the union of
both capabilities are the same set. It also checks the windows of each
capability, that `tauri.conf.json` lists both, the exact shared and
settings-only command sets, that `open_settings`, `open_plane_feed` and
`bind_harness_account` are main-only, and that `settings-window` has no plugin
or core permission. After a build, review the resolved `settings-window`
capability in `src-tauri/gen/schemas/capabilities.json` and
`acl-manifests.json` (git-ignored) before a release.

Automated coverage includes: the settings ledger state machine, fake-jetd
guard and retry cases, scope refusals with no I/O, Plane isolation of
snapshots, reviews, grants and tokens, watcher redaction, Account, usage and
Craft source validation, extension catalog projection from real-shape Codex
and Claude fixtures, notification v1→v2 migration, pruning, muting and saving
outside the gate lock, and the TypeScript adapters and pure models.

### Manual checks

Run these in `bun run tauri dev` on a Linux desktop:

- Open Settings from the sidebar, from Ctrl+, and from each deep link, both
  while the window is closed (pending target) and while it is open.
- The last pane is restored after a relaunch.
- Keyboard-only use, screen-reader names and heading order (one `h1` per
  pane).
- The Settings window at its 640×480 minimum and the main window at 900×600.
- Light and dark system themes.
- An offline Plane shows its last values as stale, and writes are disabled.
- The main window cannot invoke `load_settings`, and the Settings window
  cannot invoke `open_plane_feed` (ACL denials in the devtools console).
- The local Craft picker opens as a child of the Settings window.
- With two Planes registered, Notifications lists both, and muting one is
  kept after a relaunch.

### Not exercised

A live remote Plane, a real Craft install, a real extension change, a Plane
with a degraded audit or in read-only recovery, an OS notification from a
muted Plane, and macOS.
