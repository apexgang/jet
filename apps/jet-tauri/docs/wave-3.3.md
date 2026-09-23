# Wave 3.3: Recovery, retention and system health (Tauri / Linux)

Tasks can now be moved to Jet Trash, restored, or deleted everywhere, each
after a native review. Auto-delete rules are reviewed in Settings. The
Settings window shows Recovery snapshots, the Security audit, a diagnostic
summary, and service versions and capabilities. Restoring a snapshot,
purging snapshots and starting a new audit period each go through a review.
The main window shows at most one Plane-health notice, for the selected
task's Plane, and no gauges or charts.

## Delivered

### Jet Trash and retention

**Entry points.** "Move to Trash…" sits in the task header. **Jet Trash** is a
sidebar destination after Schedules. A trashed task's header shows an "In Jet
Trash" banner with Restore. The banner comes from `load_trash_status` for the
selected task, not from membership in the Trash list, so a task outside the
list still shows it.

**Move to Trash.** The dialog offers two modes. *Forget in Jet* is
pre-selected, following the conservative default for Project removal
(`docs/design-language.md:130`). *Delete everywhere* is an explicit choice.
The dialog names the Plane by its native label and gives the approximate
deletion date ("about", now plus the Plane's grace days). It lists the
protections the preview reported and says what stays: Harness history,
audit metadata, and copies in Recovery snapshots for up to about five weeks.
After success it shows the Plane's own `expires_at`. Delete everywhere says
"No installed Harness supports this yet; Jet records the request." When the
review showed activity, a queued message, or a Workspace Jet could not check,
Delete everywhere needs the checkbox "Stop the activity in this task and
cancel queued messages". The shell enforces this, not the webview. A preview
with an unreadable Workspace becomes an unchecked review with its own
warning. Forget stays enabled while work is running: the protection lines are
shown and `retention.live_work` is explained, because disabling it would copy
Plane policy.

**Plane sections and dates.** Jet Trash has one section per registered Plane,
each with its own state: loading, empty, ready, stale, offline, denied,
unsupported or failed. One offline Plane does not hide the others. Rows show
the reason, the trashed date and the deletion date as absolute local dates
(`dateStyle: "medium"`, `timeStyle: "short"`) next to relative wording.
`plane_transfer` entries are tombstones: "This task now lives on another
Plane. This copy can't be restored." Their Restore button is disabled.
Deletion that is due reads "Deletion is due. Jet deletes it once its
remaining work ends."

**The 256 cap.** `conversation_trash` returns at most 256 entries, soonest
deletion first, and has no cursor. A full list reads "Showing the 256 tasks
deleted soonest. More may be in Jet Trash." A list longer than 256 is not
trusted and fails the section.

**Names.** Trash entries carry no name or Project. Names come from the task
list the main window already holds for that Plane, which includes trashed
tasks. Only missing IDs go to `resolve_conversation_names`: at most 32 per
call, one connection, duplicates refused. A failed lookup shows "Name
unavailable".

**Retention line.** The task header says "Jet forgets this task after its last
activity ends." for a task created with `forget_after_final_run`. Retain shows
nothing. The policy cannot be changed after creation (no Command exists), and
the creation-time choice is deferred to 3.4, see below.

**Refresh.** `conversation.trashed`, `conversation.restored` and
`conversation.transfer_relinquished` mark that Plane's section stale. It
reloads when visible. A Plane-scope `setting.changed` or `setting.cleared`
marks the grace days stale. A new `daemonStarts` on a Plane drops only that
Plane's section, names and banner.

### Auto-delete review

`AutodeleteRules` sits in Settings › Work › Retention on the Plane the pane
shows.

- **Drafting is off by default.** `utility.autodelete_compilation` defaults to
  off in `jet-core`. Compile still records the rule, and the Plane marks it
  refused with `utility.disabled`. The rule then reads that drafting is off
  and offers "Change days", which sends `set_inactive_days` and turns it into
  a draft the user can approve. When drafting is on but no Utility account is
  bound, the section says so before the user writes anything.
- **Provider disclosure.** Above "Create draft": "Jet sends this rule's text to
  the Utility Provider selected in Settings › Agents to draft it. Task
  contents aren't sent." When drafting is unreadable, the section says so
  instead of guessing.
- **Attribution.** "Drafted by {model} via {provider}" appears only when the
  rule's Utility job produced a draft with the same number of days the rule
  shows now, and both names are displayable (64 and 96 characters). Answered
  lookups are cached per Plane binding and job, up to 256. One read issues at
  most 64 lookups, on the same connection, in order.
- **Approval binds what was shown.** A read issues opaque tokens: one for each
  draft's interpretation (Approve) and one for each approved Forget rule
  (Allow delete everywhere). The token resolves in the shell to the rule and
  the number of days the user saw. It stays the same while that binding holds,
  so the 2-second compile polling (at most 5 reloads) never invalidates an
  open dialog. The webview sends only `token_id`. Model output is never turned
  into a Command: the Plane holds the interpretation, and the shell sends
  approval of the exact rule and days it resolved.
- **One pending change per rule.** An uncertain change is kept per rule slot
  (all new rules share one slot per Plane). The same change resends its
  Command ID. A different change is refused with `autodelete.retry_mismatch`
  until the first resolves. Reads return the pending changes, so a reopened
  Settings window can still offer "Try again".
- **Candidates.** Each rule lists up to 32 candidate matches with their
  protections. Names are looked up only when the list is opened.
- Refused rules offer "Edit wording", deletion needs a confirmation, and the
  prompt has a UTF-8 byte counter bounded at 4,096 bytes.

### Recovery

The Recovery section in Settings › Safety and system shows the store state
(serving, read-only with its reason, or not reported), up to 64 verified
snapshots newest first with date, reason and size, the total count, and the
Deletion ledger status.

- **Restore** is offered only while the store is in read-only Recovery mode,
  never while it is serving. It is blocked when the Deletion ledger is corrupt
  or there is no verified snapshot. The review names the Plane and the
  snapshot date. It says that later changes are replaced, that the damaged
  data is kept beside the store, that deletions after the snapshot stay
  deleted, and that Jet may ask for an audit review afterwards.
- **Purge** ("Remove snapshots that may still contain deleted tasks?") is
  offered only on a serving store with a verified Deletion ledger and a
  trusted audit. The review gives the count and size and warns about rollback
  copies. It says: "Removed snapshots can't be recovered."
- **Snapshot names never cross IPC.** Each read hands out opaque snapshot
  tokens, reused for a name that is still listed and reset when the Plane
  identity changes. A missing token gives `recovery.snapshot_gone`.
- **Not receipt-deduplicated.** The Plane does not deduplicate either Command
  (`docs/recovery.md`). A second restore meets `recovery.not_read_only`, and a
  second purge takes another snapshot. Each review is sent at most once. An
  outcome the shell cannot confirm is recorded as unconfirmed. The section
  then re-reads the Plane and offers a new review. It never resends on its
  own.
- **State moves backwards.** After a restore the shell drops that Plane's
  snapshot tokens, Trash index, restore Command IDs and Trash reviews,
  auto-delete tokens and pending changes, and the audit export mark. It
  re-reads status so the registry's health is current. The Settings window
  reloads every section. The main window notices through its feed: a
  reconnect with a new `daemonStarts` resets that Plane's state.
- A portable Recovery bundle cannot be exported yet (see below).

### Security audit and diagnostics

**Audit viewer.** It sits in Settings › Safety › Audit, below 3.2's retention
row. It pages `security_audit` oldest first, 256 records per page, with "Load
newer records", and keeps at most 4,096 records on screen. Past that it points
to Save audit evidence. The shell redacts pages. The client ID, target
identity and target reference are withheld unless the user turns on "Show
identifiers", and the Plane ID is never sent. Turning the switch on reloads
from page 1. A Plane switch turns it off again. Revealed values must match
allowlisted shapes, and a reference shows its first 16 hex characters.
Decision labels come from `jet-core`'s audit encoding strings. An unknown code
reads "Security decision ({code})". A `pagination_stale` answer restarts once
from page 1. The audit is owner-only, so a paired client sees it as denied
for that Plane only.

**Evidence export.** "Save audit evidence" is offered in every Security state.
The shell reads status first, so an offline Plane fails before any dialog.
Then it opens the native save dialog as a child of the Settings window. The
suggested name is `jet-audit-<plane-slug>-<UTC date>.jsonl`. The file is JSON
Lines. The first line is a header `{format: "jet-audit-evidence", version: 1,
plane_label, security, exported_at_unix_ms}`, followed by one wire
`AuditEntry` per line, unredacted, because this is evidence. The shell writes
`.<name>.partial` beside the target with `O_EXCL` and mode 0600. It fsyncs the
file and renames it over the target only when complete. On any failure,
including a dropped request, the partial file is removed. An existing partial
file or symlink is never followed. No path crosses to or from the webview:
only the chosen file's name and the record count come back. The Settings
window has no `dialog:*` permission. There is one export per Plane at a time
(`audit.export_busy`), and at most 1,048,576 records.

**New audit period.** When a Plane is Security-degraded, "Start new audit
period" (`begin_audit_epoch`, ADR-0105) appears. It is enabled only after this
app saved the evidence of the degraded epoch in this session, for the same
Plane identity. The server does not require this; it is a UI rule, because
ADR-0105 says the owner exports first. The review names the Plane, the breach
and how far the saved evidence reaches. Unlike restore and purge, this
Command is receipt-deduplicated. An uncertain send stays retryable with the
same Command ID ("Try again"), even after the dialog or the whole Settings
window is closed and reopened: the shell reports the pending request in the
health read (`pendingEpoch`), and preparing a new period returns that same
review (`unconfirmed: true`) until its outcome is known, whatever the audit
now shows. The review names the degraded epoch; before a first send the shell
reads the Plane again and refuses (`audit.review_stale`) unless that epoch is
still the degraded one and its evidence mark still stands, because the
Command closes whichever epoch is current.

**After a restore.** A restore whose outcome is confirmed, unconfirmed (lost
reply) or names another snapshot may have moved the store backwards, so the
shell drops every cache of that Plane's store in all three cases: snapshot
tokens, the evidence mark, the Trash index, Trash reviews, restore IDs,
auto-delete tokens and pending changes, and every other Recovery review
(including an unresolved new audit period, which would otherwise run against
an audit whose evidence was never saved).

**Diagnostics.** Settings › Safety › Diagnostics shows a summary the client
builds for support, with a Copy button. The copy runs from the click handler
and falls back to selecting the text. Every line comes from an allowlisted
field or enum: app version, supported and negotiated protocol, Jet service
version and starts, platform, tool presence, Craft count, secure-storage
state, degraded conditions, store and Deletion ledger state, audit state,
temporary-file budget, Trash grace days, sections that failed to load, and
the last 20 stable error codes seen in this Settings window. It has no Plane
label, ID, path, prompt, Craft name or native version text. A version that is
not a plain version token reads "unrecognized". The Diagnostic log stays on
the Plane's computer, and the section says so.

### System health

**Main window.** One `PlaneHealthNotice` appears above the timeline, for the
selected task's Plane only, and only while that Plane is read-only,
ledger-corrupt, Security-degraded, or recently refused work for low disk
space. Priority follows that order, and one notice is shown at a time. The
notice adds one item to Needs attention, and choosing Needs attention focuses
it. The feed's `connected` snapshot carries a compact health summary (store,
ledger, security). `resumed` changes nothing. Refusals seen on requests
(`recovery.read_only`, `recovery.deletion_ledger_corrupt`,
`security.audit_degraded`, `storage.disk_pressure`) are recorded against the
Plane that refused them. A disk-pressure refusal on one Plane never shows on
another Plane's task. It reads "Low disk space on {Plane} stopped new work at
{time}." and offers "Free space", a link to Safety › Storage, and Dismiss. The next admitted request on that Plane clears it. The notice links
to Safety › Recovery and Safety › Audit for the other conditions.

**Settings window.**

- *Versions and capabilities* shows the Jet service version, starts and start
  time, app version, "Supports protocol 1.43", the negotiated minor when
  known, platform, external tools with versions, Crafts, the credential store
  and degraded conditions. Each item that has a fix links to its 3.2 section.
  "Check again" asks the Plane for a fresh capability observation.
- *Storage health* sits inside 3.2's Storage section. It shows the temporary
  file budget, the last low-disk refusal seen in this window, and "Free
  disposable space". That runs one bounded Artifact collection per Plane at a
  time, from either window.

Resource budgets and power are not shown. No counter, gauge or chart was
added to the main workspace. Health is a condition with a repair, not a
metric.

**Lost activity.** The Run tab shows a *Recovery needed* panel when the
activity's lifecycle is `lost` or supervision says it needs attention. The
panel has no action buttons (see `jet_client_orphaned_executions`).

### Settings deep links added

| Codes | Target |
|---|---|
| `recovery.read_only` (also from a Trash restore), `recovery.deletion_ledger_corrupt`, `recovery.restore_failed`, `recovery.snapshot_gone`, `recovery.purge_unavailable`, `recovery.not_read_only_local` | Safety › Recovery |
| `audit.export_required` (added beside 3.2's `security.audit_degraded`) | Safety › Audit |
| `security.gap_unknown` | Safety › Diagnostics |

In Versions, a degraded condition links to its section: no Harness available
to Agents › Harnesses, locked or unavailable secure storage to Agents ›
Accounts. A missing external tool links nowhere.

Links render only for sections that have landed. `diagnostics`, `versions`
and `recovery` were added to the Safety pane in the order execution,
permissions, storage, recovery, diagnostics, audit, versions.

## Command-ID and review rules

All reviews live in a per-Plane native ledger (`src-tauri/src/jet/ledger.rs`):
at most 256 reviews, each usable for 10 minutes. It prunes on insert and
evicts only unattempted reviews. A review's ID is its Command ID. While one
review for a scope is attempted and unresolved, no other review for that
scope can be issued or attempted (`*.request_unresolved`). A lost IPC reply is
answered from the recorded outcome. A review is bound to the Plane handle and
Plane identity it was made against, and runs only there
(`client.review_plane_mismatch`, `plane.review_moved`).

- **Trash reviews.** The first attempt locks the mode
  (`retention.mode_locked`). Delete everywhere needs the stop
  acknowledgement when the review requires it
  (`retention.stop_unacknowledged`). A first Delete everywhere re-reads the
  task. New activity, a new queued message the user did not acknowledge, or a
  newly unreadable Workspace makes the review stale (`retention.review_stale`,
  "Review again"). After a failure the review stays attempted, and "Try
  again" resends the same Command ID and mode. A first attempt that could not
  connect, or whose re-read failed, is recorded as refused, because nothing
  was sent.
- **Restore keys.** A restore Command ID is keyed by (Plane, task, the
  `trashed_at` of the Trash entry the shell itself read). A task trashed again
  gets a new ID and can never replay an old receipt. The index holds up to
  512 entries per Plane. An evicted entry makes restore answer
  `retention.trash_unknown` until the next read.
- **Rule slots.** There is one pending body per auto-delete rule and one for
  new rules per Plane, with at most 256 pending changes. They are keyed by
  Plane binding, so a Plane whose identity changed never reuses a Command ID
  or token.
- **Recovery actions.** Restore, purge and new epoch share the recovery
  ledger, one scope per action per Plane. Restore and purge are sent at most
  once (a retry of an attempted review answers `client.review_used` without
  contacting the Plane). A new epoch is retried with the same ID.
- **A retry whose Plane moved** (Trash or new epoch) returns an error and
  records nothing: its first send may have applied, so it stays unresolved.
- **Local refusals** from these checks return a typed refused outcome with the
  Plane ID, not an IPC error. The webview treats an IPC error as uncertain.

## Permissions

| Command | Main window | Settings window |
|---|---|---|
| `load_trash`, `load_trash_status`, `preview_trash`, `trash_conversation`, `restore_conversation` | yes | – |
| `resolve_conversation_names`, `collect_disposable_storage` | yes | yes |
| `load_system_health`, `load_autodelete_rules`, `change_autodelete_rule`, `prepare_recovery_action`, `execute_recovery_action`, `load_security_audit`, `export_security_audit` | – | yes |

The grants follow spec §5 exactly, and the manifest test in
`src-tauri/src/jet/mod.rs` pins them. Recovery, the
audit and auto-delete sit in the Settings window, which never renders agent
output. Every command has its autogenerated TOML in
`src-tauri/permissions/autogenerated/`.

## Remaining backend dependencies

| ID | Missing surface | Evidence | UI disclosure |
| --- | --- | --- | --- |
| `jet_client_orphaned_executions` | Public `jet-client` methods for `orphaned_executions` and `resolve_execution` | `packages/jet-client/src/connection/mod.rs:220` (`pub(crate) query`); no method in `requests/run.rs`. `ResolveExecution` needs the inspected `instance` (`packages/jet-protocol/src/conversation/command.rs:171-179`) | Run tab: the activity's process can't be inspected or ended from this app yet |
| `disk_pressure_status` | A Query or degraded condition that reports storage pressure | `packages/jet-protocol/src/capability.rs:150-170`, `message/mod.rs:167-195`; pressure is refused per admission only (`docs/disk-pressure.md:9-12`) | Pressure is shown only as "last refused at {time}", never as a live gauge |
| `diagnostic_log_query` | Reading the Diagnostic log over the protocol | `docs/diagnostics.md:3-8` | Diagnostics says the log stays on the Plane's computer |
| `recovery_bundle_transport` | Portable Recovery bundle export and import over the JSON transport | `docs/recovery-bundles.md:3-6` | "Exporting a portable Recovery bundle isn't available in this app yet." |
| `native_deletion_capability` | Whether a Harness supports native deletion | `packages/jet-protocol/src/capability.rs:134-144`; `docs/retention.md:24-28` | "No installed Harness supports this yet; Jet records the request." |
| `client_negotiated_minor_accessor` (3.1's ID) | Public negotiated minor | `packages/jet-client/src/connection/mod.rs:216` | Versions shows "Supports protocol 1.43", plus the negotiated minor when 3.1's per-Plane knowledge has it |
| `trash_entry_labels` (non-blocking) | Trash and candidate entries carry no name or Project | `packages/jet-protocol/src/retention.rs:34-43`, `autodelete.rs:74-82` | Names come from the task list, then the native lookup; a failure shows "Name unavailable" |
| `trash_pagination` (non-blocking) | `conversation_trash` has no cursor beyond 256 | `packages/jet-store/src/conversation/trash.rs:83, 128-143` | "Showing the 256 tasks deleted soonest. More may be in Jet Trash." |
| `retention_policy_command` (non-blocking) | No Command changes an existing Conversation's `RetentionPolicy` | `conversation/command.rs:205-210` sets it only at creation | The policy is shown read-only in the task header |

Deferred client work: the creation-time retention choice. The surface exists
(`create_conversation(…, retention)`,
`packages/jet-client/src/requests/conversation.rs:178-181`), but the shell
sends `RetentionPolicy::Retain` (`src-tauri/src/jet/conversations.rs`).
Neither the Swift app nor design-language asks for it, so it is a 3.4 parity
row. Archive does not apply: the protocol has no archive Command or state.
The Linux shell has no menu bar, so "Move to Trash…" and a Help entry to
Diagnostics in a menu are 3.4 parity items.

## Latent fixes included

- **D4.** The main window refreshed Jet Trash on `conversation.trashed` but not
  on `conversation.restored`. It now handles both, plus
  `conversation.transfer_relinquished`.
- **D1** (`build.rs` omitted six commands) and the manifest consistency test
  had already landed with 3.1. This wave extended the test to its own
  commands and to the per-window split.
- 3.3 destructive copy uses only the native Plane label (D5), never the
  fixture's Plane name.

## Verification

Run from `apps/jet-tauri` with the pinned Rust 1.98.1 toolchain:

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

Results on 2026-09-23: svelte-check 312 files with 0 errors and 0 warnings;
Vitest 300 tests in 28 files; the static build succeeded; rustfmt and clippy
were clean; Rust 252 tests passed and 3 ignored (the 3.1 live matrix, the real
Secret Service probe, and the 3.3 live run below, which was run by hand); the
debug Debian bundle `Jet_0.1.0_amd64.deb` (about 85 MB) built.

Automated coverage includes the review ledger (lifetime, capacity, eviction,
unresolved guard, Plane binding, injected clock), Trash mode lock, stop
acknowledgement, stale re-read and re-trash sequence, restore keys, rule slots
and token stability, attribution caps, snapshot tokens, restore and purge
outcomes, audit redaction, paging checks, the export file's partial-file
handling and `O_EXCL` refusal, the export gate on a new epoch, and the
TypeScript sessions and models with mocked IPC and fake timers for polling.

### Live run against a scratch Plane

`src-tauri/src/jet/live_system.rs` is ignored by default. It starts `jetd
serve` in a new directory under `$XDG_RUNTIME_DIR` and drives the shell's own
command functions (the bodies behind the Tauri commands) against it. `~/.jet`
and the login keyring are never touched. It needs the `sqlite3` shell to find
the pages it damages.

```sh
cargo build --manifest-path ../../packages/Cargo.toml -p jet-daemon --bin jetd
JET_E2E_JETD=$(realpath ../../packages/target/debug/jetd) cargo test \
    --manifest-path src-tauri/Cargo.toml live_system -- --ignored --nocapture
```

Outcome on 2026-09-23 (jetd `0.2.0` built from this branch's `packages/`):

| Check | Result |
|---|---|
| Forget, then restore; forget again, restore again | Passed. Each forget went through a new review. The Plane gave the second trash its own `trashed_at`, so the second restore used a new Command ID. The task left the Trash list both times. |
| Delete everywhere on an idle task | Passed. The entry reads `delete_everywhere` and needed no stop acknowledgement. |
| Compile a rule with drafting off, set days, approve | Passed. The compile was recorded, the reload showed `refused {utility.disabled}` with drafting `enabled: false`, "Change days" to 30 produced a draft with an approve token, and approving that token gave an approved rule with its delete-everywhere token. |
| Audit paging and export | Passed. 8 records on one complete page. The evidence file had 9 lines (header plus 8), mode 0600, and no `.partial` file was left. |
| Purge on the scratch Plane | Passed. The review listed 1 snapshot. The outcome was `purged {removedCount: 0}`: jetd took a maintenance snapshot and kept the day's newest. |
| Read-only store and restore | Passed. With jetd stopped, the test zeroed the `conversations` table's page in the scratch store. On restart jetd served it read-only (`integrity_check_failed`). The restore review named the maintenance snapshot. The outcome was `restored` with `replacedName: plane.sqlite3.damaged-<ms>`, the store served again, and the snapshot tokens were new. |
| Audit after the restore | Security-degraded with `head_not_in_store`, as `docs/recovery.md` describes. A new audit period was refused with `audit.export_required` until the evidence was saved. The review then showed `exportedThrough: 8`, the epoch began (`epoch 2`), and security read trusted. |

The first run damaged every other page and hit the schema, and jetd then
refused to open the store at all ("database disk image is malformed"). That
case is outside the protocol: no client can reach a Plane that does not start.

### Visual and accessibility checks

The real Settings route ran in `bun run dev` under Chromium with a temporary,
uncommitted `mockIPC` harness. It served the JSON the live run above returned
(serving, read-only and degraded health, the audit page, auto-delete rules,
the Plane list). Every other command answered `transport.offline`. The
embedded axe-core 4.12.1 audit ran at 820×620 in light and dark on:

- the Safety pane (serving, read-only, degraded) and the Work pane with
  Auto-delete rules: 0 violations.
- the purge and restore review dialogs: 0 violations. For the purge dialog,
  axe could not compute the contrast of `.dialog-alert` on its `color-mix`
  background ("incomplete"). The screenshots show it readable in both themes.

The first audit flagged `aria-label` on the diagnostic summary's `<pre>`
(`aria-prohibited-attr`), which is now removed. At 640 px (the Settings
minimum) nothing scrolls sideways, and the audit table fits.

Style polish in this slice: the Move to Trash mode cards no longer need
`!important`. The chosen mode keeps a `Highlight` border under
`forced-colors`. The recovery and storage warnings get a transparent border
that a forced palette paints. The header's retention line stays on one line
and has the full sentence as its title.

### Not verified

- The main window's Jet Trash destination, task banner and Move to Trash
  dialog in a browser or the running app. They are covered by the Vitest
  sessions and the live run of the same shell functions, not clicked through.
- Delete everywhere on a task with running activity, which needs an
  installed Harness. The stop acknowledgement and stale re-read are covered by
  the Rust ledger tests with a fake jetd.
- A live Utility Provider drafting a rule, so attribution was not seen live.
- A real low-disk refusal, a remote Plane, a paired (non-owner) client seeing
  the audit as denied, and `recovery.restore_failed`.
- Clipboard copy in WebKitGTK and the native save dialog. The live run called
  the export after the dialog step.
- The running Tauri app itself. The debug `.deb` was built but not launched.
- macOS, RPM and AppImage packaging.
