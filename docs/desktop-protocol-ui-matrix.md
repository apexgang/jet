# Desktop protocol to UI capability matrix

Status: Wave 0.1 foundation for protocol major 1, generated at minor 43.

This matrix binds the confirmed desktop design to the current Jet client
protocol. It covers every Wave 1 and Wave 2 interaction and records later
screens where the protocol already exists. A row marked as a backend dependency
must stay disabled or absent in the client. Local state cannot imitate it.

The protocol source is `packages/jet-protocol`. Swift declarations in
`apps/jet/jet/Protocol/JetModels.swift` are generated models, not decoders.
Every incoming control frame must pass `jet-v1.schema.json` validation before a
typed client constructs these values.

## Rules shared by every row

- Every Command uses a stable `command_id`. A retry reuses it only with the
  byte-equivalent Command body. Disconnecting never implies rollback.
- A snapshot or first page carries a Plane Event cursor. The client subscribes
  strictly after that cursor and keeps one cursor per Plane.
- Later pages use their returned opaque cursor. `pagination_stale` restarts from
  the first page instead of mixing snapshots.
- The negotiated protocol minor gates operation availability. `capabilities`
  supplies runtime facts such as Harnesses, external tools, credential-store
  state, and degraded conditions.
- The client branches only on `ErrorCategory`, stable `code`, `retryable`,
  `revision_conflict`, `restart`, and `recovery_actions`. Native error text is
  diagnostic data, never control flow.
- Event order comes from the Plane-local decimal `sequence`. Timestamps are
  display metadata.

## Shell and onboarding

| Interface | Read path | Mutation path | Cursor, revision, or capability rule | Status |
| --- | --- | --- | --- | --- |
| Connect to a Plane | Handshake, then `status` and `capabilities` | None | Authenticate before state access. Keep the negotiated major and minor for the connection | Supported |
| First-launch status | `status`, `capabilities`, `projects`, `account_bindings` | `bind_account`, `register_project` after their previews or credential flow | Credentials stay in Keychain. The protocol carries only `CredentialSource` and non-secret metadata | Supported |
| List and select Projects | `projects` | None for selection | Selection is disposable client state. Project identity and path remain Plane-owned | Supported |
| Add a Project | `preview_project` | `register_project` | Show the canonical preview before the Command. Refresh capabilities when Git support may have changed | Supported |
| Remove a Project | `preview_project_removal` | `remove_project` | The Command must carry the exact preview binding, disposal choice, and typed Project-name confirmation | Supported, minor-gated |
| Recent Conversations | `conversations`, then `next_conversations` | None | Preserve the first-page cursor and restart on `pagination_stale` | Supported |
| Search | One `search` Query per connected Plane | None | Merge bounded results client-side and keep Plane provenance. Never invent cross-Plane order | Supported |
| Needs attention | Selected items can use `run_execution`, `turn_queue`, and Events | Depends on the item | No bounded aggregate Query lists attention items across a Plane | Backend dependency: `needs_attention_query` |
| Pinned Conversations | None | None | ADR-0034 requires shared or private authoritative layouts, but no generated Query or Command exposes them | Backend dependency: `conversation_layout` |
| Schedules | `scheduled_tasks` | `create_schedule`, `cancel_schedule` | Schedule identity is durable. The protocol owns queued schedule input | Supported for Wave 3 |
| Plane and pairing management | `status`, `capabilities`, `pairing` | `set_pairing_gate`, `open_pairing`, `claim_pairing`, `confirm_pairing`, `complete_pairing`, access and revoke Commands | Private Client keys stay in Keychain. Pairing and SSH authentication are distinct | Supported for Wave 3 |

## Conversation and composer

| Interface | Read path | Mutation path | Cursor, revision, or capability rule | Status |
| --- | --- | --- | --- | --- |
| Open a Conversation | `conversation`, then `events` after its cursor | None | Replace the whole snapshot after `cursor_expired` or `cursor_ahead` | Supported |
| New task | `projects`, `capabilities`, optional settings | `create_conversation`, then `start_run`, `start_visa_run`, or `start_no_visa_run` | Show Project, Harness, and Runs on before starting. Never persist an optimistic Conversation | Supported |
| Timeline | `events`; known or opaque Presentation blocks from Event payloads | None | Preserve raw JSON. Render text as text and sanitize Markdown. Unknown blocks get a generic non-executable view | Supported after the Wave 0.2 validator |
| Send while idle or active | `turn_queue` and Events | `submit_turn` | The Plane assigns order. The client does not use a Revision for append-only user input | Supported |
| Withdraw queued input | `turn_queue` | `withdraw_turn` | Only the caller's queued user Turn is eligible. Queue position comes from vector order | Supported |
| Harness approval | `run_execution` with `waiting_for_approval`, plus structured Event Presentation | No generic approval-decision Command exists | Both desktop clients display bounded structured requests and disclose the unavailable decision action. Do not route the choice through a local callback or raw Harness channel | Backend dependency: `approval_decision_command` |
| No-Visa remote-tool approval | `remote_tool_review` | `review_remote_tool` | Decision applies to the exact stored `client_id` and `operation_id` | Supported, separate from Harness approval |
| Retry an automatic-review denial | Events and `run_execution` | `authorize_approval_retry` | The Command names the Run and stored review. It cannot substitute another action | Supported, minor-gated |
| Interrupt current Turn | `run_execution`, `turn_queue` | `interrupt_turn` | This ends the claimed Turn and preserves the Run. It is not request cancellation | Supported |
| Stop Run | `run_execution` | `stop_run` | This ends the Run and is distinct from Interrupt Turn | Supported |
| Rename Conversation or Run | `conversation` | `set_conversation_name`, `set_run_name` | Send the observed entity Revision. On conflict, replace with `safe_state` before offering retry | Supported |
| Fork | `conversation`, checkpoints exposed through change data | `fork_conversation` | The Command names the source Run and one-based checkpoint Turn | Supported |
| Handoff to another Harness | Conversation and Change state | `handoff_conversation` | Creates a new Conversation. It is not a move or Plane transfer | Supported |
| Completion | `conversation`, `run_execution`, Events | None | A terminal lifecycle has no live `RunActivity` | Supported |

## Work panel and delivery

| Interface | Read path | Mutation path | Cursor, revision, or capability rule | Status |
| --- | --- | --- | --- | --- |
| Queue | `turn_queue` | `submit_turn`, `withdraw_turn` | At most 128 entries. Vector order is authoritative | Supported |
| Run details | `run_execution` | `interrupt_turn`, `stop_run`, recovery Commands where applicable | Lifecycle and activity are separate. Preserve process identity as historical detail | Supported |
| Changed files and diff | `change_diff`, `next_change_diff` | None | Page through file metadata. Load bounded patch Artifacts rather than a full large diff | Supported |
| Patch Artifact chunks | `change_artifact` with a byte offset | None | Advance through bounded chunks and verify declared size and SHA-256 before treating an Artifact as complete | Supported |
| Files | `project_entry`, `editable_file` | `apply_user_edit` | Paths are relative to a registered Project or Workspace. Edits send the exact `FileRevision` | Supported |
| Structured review comments | `editable_file` and Change data | `submit_review` | The complete comment batch becomes one user Turn | Supported |
| Workspace terminals | `workspace_terminals` | `open_terminal`, attach and resize client messages, `close_terminal` | Terminal streams use byte credit and explicit gap or finish controls. Disconnect does not close a terminal | Supported after Wave 0.2 streaming |
| Commit, push, or draft pull request | `git_deliveries`, Change checkpoint data | `deliver_git`, `acknowledge_git_delivery` | Gate on Git capability. Preserve `outcome_unknown`; never retry uncertain external work automatically | Supported |
| Completion notification | Run Events | None | Notification preference and delivery are client-local. A notification never changes Run state | Client-local |

## Settings, retention, and system state

| Interface | Read path | Mutation path | Cursor, revision, or capability rule | Status |
| --- | --- | --- | --- | --- |
| Jet settings | `settings` | `set_setting`, `clear_setting` | Values resolve by Plane, Project, and Conversation scope. Policy enforcement remains in `jetd` | Supported for Wave 3 |
| App appearance, window restoration, local notification preference | None | None | Non-sensitive, client-owned preferences only | Client-local |
| Harnesses and accounts | `capabilities`, `account_bindings`, extension Queries | Account and extension Commands | Respect credential-store degraded states. Never copy credentials between Planes | Supported for Wave 3 |
| Trash and retention | `conversation_trash`, `retention_preview`, `autodelete_rules` | forget, delete-everywhere, restore, and autodelete Commands | Show affected Plane and recovery window. Model output authorizes nothing | Supported for Wave 3 |
| Recovery | `status`, `orphaned_executions` | `resolve_execution`, `restore_recovery_snapshot`, `purge_recovery_snapshots` | Use exact execution instance. A lost Run remains attached to its Conversation | Supported for Wave 3 |
| Security and diagnostics | `security_audit`, `status`, `capabilities` | `begin_audit_epoch` where allowed | Redact prompts, file content, terminal output, and secrets | Supported for Wave 3 |
| Plane transfer | No complete GUI Query or transfer transport | None | Existing documents state that GUI bundle transport remains separate work | Backend dependency: `plane_transfer_gui` |

## Error and recovery presentation

| State | Stable protocol evidence | Client behavior |
| --- | --- | --- |
| Offline | Transport loss or `unavailable` | Keep the last trustworthy state visibly stale. Disable Commands until authenticated reconnect completes |
| Stale Event cursor | `restart.reason` is `cursor_expired` or `cursor_ahead` | Fetch a fresh fenced snapshot, replace derived state, then subscribe after its cursor |
| Stale page | `restart.reason` is `pagination_stale` | Discard the page chain and restart at the first page |
| Denied | `unauthorized` plus stable code, or an approval-denial Event | Explain the denied action and scope. Offer only a named protocol recovery action |
| Revision conflict | `conflict` with `revision_conflict.safe_state` | Replace the affected entity state before the user deliberately retries |
| Unsupported | Negotiated minor refusal, `incompatible`, missing external tool, missing Harness, or degraded condition | Keep the rest of the product available and explain the exact missing capability |
| Outcome unknown | `outcome_unknown` | Show that an external effect may have happened. Inspect the durable outcome or acknowledge it; do not replay |
| Rate limited | `rate_limited` and `retryable` | Preserve the draft and follow the stable recovery metadata. Do not switch Provider, account, or Plane silently |
| Internal failure | `internal` with redacted safe message | Preserve state, offer a safe retry only when `retryable`, and keep native detail out of the interface |

## Fixture coverage

The shared corpus is `fixtures/desktop/presentation-v1.json`. The Swift loader
rejects invalid cross-field combinations before previews or tests can use them.

| Fixture | Protocol path or recorded gap |
| --- | --- |
| `first-launch` | `status`, `capabilities`, `projects`, `account_bindings` |
| `ready-new-task` | `create_conversation`, `start_run` |
| `active-run` | `conversation`, `run_execution`, `events`, Run control Commands |
| `queued-turns` | `turn_queue`, `submit_turn`, `withdraw_turn` |
| `approval-needed` | Display through Run state and Events; records `approval_decision_command` as missing |
| `completed-run` | `change_diff`, `git_deliveries`, `deliver_git` |
| `offline-cached-conversation` | No fabricated Query result; cached state is marked stale |
| `expired-event-cursor` | `cursor_expired`, fresh `conversation`, then `events` |
| `approval-denied` | `unauthorized`, `authorize_approval_retry` |
| `unsupported-git-delivery` | `capabilities` reports Git missing; `deliver_git` stays disabled |
| `lost-run-recovery` | `run_execution`, `orphaned_executions`, `resolve_execution` |

## Verification hooks

- `apps/jet/scripts/check-generated-contracts.sh` runs the repository's
  contract regeneration diff. It fails when the Swift models or schema are
  stale.
- `DesktopFixtureCorpusTests` loads the shared corpus through the same Swift
  value types previews will use and checks fail-closed format handling.
- The Linux verification path must call the same `contracts-check` recipe and
  consume this exact corpus when its foundation slice begins. Do not copy the
  JSON into the Tauri tree.

Applied controls: ASVS 1.5.2 requires allowlisted deserialization; ASVS 2.1.1
through 2.1.3 require the validation and limit rules documented here; ASVS
2.2.1 through 2.2.3 require structural and cross-field validation while keeping
authority in the trusted service layer.
