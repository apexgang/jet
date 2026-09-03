# feature list

## heeka

- control this Plane from another GUI client, including iPhone (codex feature)
- control other Planes from this GUI client (codex feature)
- control other Planes over SSH (codex feature)
- automatic Git branches, commits, pushes, and GitHub draft PRs, independently controllable from the app; branch naming is configurable, while commit and PR text use the Utility model plus optional natural-language instructions
- work trees (codex feature)
- approve for me (codex feature)
- make Approve for me global across Harnesses on each Plane without widening permissions; use a separate same-Provider reviewer, deterministic critical-deny rules, fail-closed results, visible rationale, bounded denials, and exact-action user retry
- keyboard shortcuts! Default + customizable
- see usage per Conversation, Harness, Provider account, and Model with deduplicated adaptive polling, 90-day raw samples, one-year hourly aggregates, and daily long-term aggregates
- dictation
- focus filters
- keychain
- apple shortcuts
- energy efficiency
- apply a per-Plane low-power concurrency budget to new background Runs and subagents without killing active work; use native Harness limits where available
- scheduled tasks anchored to their original IANA time zone with deterministic DST and missed-firing behavior (codex feature)
- summary block (codex feature)
- side panel: files and user-attributed direct edits, reconnectable Workspace terminals independent from Runs, structured review submissions, and a side-chat view of the same Conversation and Turn queue
- reorder and pin Conversations using a Plane-shared layout by default, with optional per-client private layouts; reorder Runs locally in the GUI without Run pins
- fork Conversations (codex feature)
- Handoff to another Harness automatically
- subagents view (codex feature)
- search (codex feature)
- completion notifications (with customizable sounds)
- toggle attention and completion sound effects, choose device-local sound collections, route playback to the current or another connected Plane, and fall back to the native OS notification sound on playback error
- browse, install, update, and remove skills and MCP servers in the Jet GUI through Harness-native catalogs, configured native marketplaces, and explicit Git URLs while preserving native formats; no Jet marketplace in v1
- discover and install Jet Crafts from `jet-craft-<slug>` GitHub repositories containing `.jet/craft-spec.toml`
- automatically name Conversations and Runs through the Home Plane's Utility Account binding; manual names win, while terminal titles name Managed processes only
- see current Harness plan status in a Conversation (codex feature)
- see current diff (codex feature)
- see final diff and changed files (codex feature)
- unregister Projects without touching their files, or remove a Project from a Plane through a guarded, two-phase, user-confirmed destructive action that is unavailable to agents and Harnesses
- autodelete Conversations through configured rules

## arasanya

- one tree across connected Planes: Plane → Project → Conversation or live Run (Herdr-style sidebar)
- any GUI client sees and controls Runs on every paired Plane, in any client-to-Plane combination (Tailscale plus SSH only)
- toggle per Conversation between Visa mode (deploy the harness on its destination Plane for exact native behavior) and No-Visa mode (keep the harness on its Home Plane while its `jetd`, not the GUI, directly exposes paired destination Planes over SSH), maximizing functional parity where the harness permits it; mobile cannot originate No-Visa work
- detect Harness processes started outside the app, import their native conversations, and resume them as Jet-managed Runs
- allow several GUI clients to control one Conversation simultaneously without locks; `jetd` sequences every admitted user, schedule, and Auto-continue input, never replaces user turns, gives schedule and Auto-continue one replaceable slot each, uses Revisions only for conflict-sensitive Commands, and exposes withdrawal and Interrupt turn separately
- keep GUI event windows bounded and cursor-replayable; preserve semantic control events under pressure while explicitly coalescing or truncating only raw terminal and redundant progress output
- preserve unparsed Harness output losslessly in a bounded Run-helper spool and apply backpressure at its limit; terminal helpers may use an explicit rolling-output gap
- auto-continue after rate limit: defaults per account, one-shot enable per chat, customizable message, configurable delay and max retries
- accounts model: account ≠ Plane (work vs personal subscriptions); one panel with every account, limit fill and reset times; usage history, diagrams, plots, and stats
- Managed-process labels from native terminal titles in the sidebar; Conversation and Run names remain independent
- reorder live Runs in the GUI; Runs are not pinnable
- live sidebar: Runs and Managed processes appear and disappear with their lifecycles, with no manual cleanup
- diffs side-by-side (vs code style): current working tree, per-turn, and any commit from the full git history (all branches)
- harness extensibility through Jet Crafts: claude code + codex bundled first, opencode / pi / hermes installable later

## agreed principles

- one `jetd` daemon per desktop Plane plus thin GUIs (macOS and iOS in Swift; Linux in Tauri; iOS is remote-only); Windows is outside v1
- enforce the one-daemon invariant with an owner-only lifetime lock and validated process, version, and installation-channel metadata; a competing daemon refuses to start
- every Conversation has one authoritative Home Plane; Visa Runs execute there, No-Visa keeps its Harness there, and changing authority requires explicit Plane transfer rather than state replication
- acknowledged control changes are durably committed; external work uses a recoverable Effect outbox, while replayable Harness output may use small bounded batches
- keep authoritative SQLite state in WAL mode with `synchronous=FULL`; meet throughput targets through bounded batching rather than acknowledging power-loss-vulnerable commits
- degrade safely under disk pressure: protect a 2 GiB or 5% free-space reserve, collect only eligible disposable data, preserve protected work, and reject new Runs or large writes while reads, export, and cleanup remain available
- distinguish interrupting the active turn from stopping its Run, preserve partial output and Workspace changes, and record forced termination explicitly
- attribute Workspace changes from evidence rather than process guesses, using external or unknown when Jet cannot prove a user, terminal, or Harness origin
- store mutable settings transactionally in SQLite with built-in, Plane, Project, and Conversation precedence; keep `~/.jet/config.toml` limited to bootstrap configuration
- treat Plane-wide settings honestly: the GUI may apply them to several connected Planes with per-Plane results, but Jet provides no atomic fleet-wide setting in v1
- require a registered Git Project for managed v1 Runs; show unsupported external Conversations as metadata until the user maps or registers their repository
- expose explicit point-in-time Plane capability snapshots and revalidate requirements at command execution without periodic polling
- expose no network listener in v1: use owner-only local IPC and authenticated SSH standard I/O for remote clients
- use the same bounded framed Jet protocol on both transports: strict JSON control frames and raw binary terminal or Artifact frames, with codec negotiation reserving a future benchmark-driven binary encoding but no gRPC in v1
- authenticate every connection through a restricted, version-negotiated handshake; remote clients sign a fresh pairing challenge and no reusable bearer session token exists
- multiplex prioritized control, event, terminal, and Artifact streams over one GUI connection per Plane, with independent byte-credit flow control for binary data
- make initial state synchronization gap-free by fencing every snapshot or first page with its Plane Event cursor; use opaque expiring keyset cursors and explicitly restart stale pagination
- retain Command deduplication records for 30 days, returning the original result for an identical retry and rejecting reused or expired identities rather than risking duplicate execution
- use UUID strings, signed Unix-millisecond timestamps, decimal-string sequences and revisions, integer-millisecond durations, and explicit IANA schedule zones on the wire
- evolve compatible protocol minors through optional fields while rejecting unknown message kinds, Commands, and security-sensitive variants; preserve opaque Presentation blocks for generic rendering
- preserve one negotiated protocol major for the full lifetime of every active execution; stage an incompatible core update until affected work finishes or the user explicitly stops it, except for signed Craft security revocation
- treat transport request cancellation separately from committed domain Commands and Effects; only explicit Interrupt turn or Stop Run Commands change active execution
- reconnect `jetd` to validated owner-only helper descriptors after restart; expose unmatched live helpers as Orphaned executions requiring user action
- pin each Run to an exact Craft artifact digest, stage updates for new Runs, and never replace the Craft underneath active work
- select Utility-model work through one Plane-wide Utility Account binding without silent cross-Provider fallback; use deterministic text fallbacks and fail natural-language deletion closed
- compile natural-language Autodelete rules into reviewed deterministic rules; edits require renewed approval and model output never directly deletes data
- distinguish Craft feature support, Jet-enforced broker permissions, and disclosed host access, requiring renewed confirmation for every expansion
- address ordinary files by Project or Workspace identity plus validated relative path; reserve canonical absolute-path grants for explicit interactive registration and import
- recover an expired Event cursor through a fresh fenced snapshot instead of returning a partial replay
- drain `jetd` updates without ending active executions, leaving their `jetfueld` helpers alive and persisting unfinished Effects for recovery
- after an operating-system reboot, mark interrupted Runs lost and require an explicit Resume into a new Run unless an existing Scheduled task or Auto-continue policy independently authorizes later work
- run an explicitly installed third-party Jet Craft as trusted same-user code in v1, with prominent provenance and trust disclosure, sanitized environment, isolated state, and brokered Jet operations; do not claim portable mandatory sandboxing
- reserve Orphaned-execution adoption and termination for interactive users after identity and Workspace validation
- minimize and disclose Utility-model inputs, require explicit cross-Provider enablement, and treat every generated result as untrusted data without tools or Command authority
- allow only one active Jet-managed Run in a Project's Local checkout; keep isolated Workspaces as the default and require explicit enablement for schedules in the Local checkout
- import dirty starting state from an immutable user-selected snapshot, excluding ignored files by default and never following symbolic links
- prevent old Recovery snapshots from resurrecting deletions through a content-free Deletion ledger and a separate user-only Recovery purge
- support ordinary non-bare Git repositories in v1 with bounded behavior for sparse checkouts, submodules, nested repositories, and optional Git LFS
- provide authenticated, encrypted-by-default Recovery bundles using binary `age` v1 with only X25519 recipients or scrypt passphrases; allow only explicit warned unencrypted export, verify before staging, keep credentials and trust identities out, and enforce the encryption dependency's binary-size budget
- import portable Recovery bundles only as Recovered copies with new Conversation identities and disabled schedules; only two-phase Plane transfer or a fence-validated local snapshot restore may preserve authority
- separate a transfer's 30-day content tombstone from its permanent content-free Authority fence; stale state with a retired authority epoch can only become a recovered copy with new Conversation identities and disabled schedules
- keep Conversation data owner-only and rely on full-disk encryption in v1; credentials remain in Keychain, Secret Service, memory, SSH agent, or external helpers and never in `~/.jet`
- when Linux Secret Service is absent, let the GUI explicitly invoke OS-mediated installation and verify secure storage before enabling durable Pairing; the core never invokes the package manager
- use PackageKit's OS-authenticated session interface for supported Linux secure-storage installation, never capture privilege passwords or construct privileged shell commands, and fall back to exact manual instructions when unsupported
- execute No-Visa operations as typed Plane-and-Workspace-scoped requests; use argument arrays by default and reserve shell syntax for a separately labelled and reviewed operation
- authenticate No-Visa requests with both the desktop installation identity and originating Actor; disabling or revoking Pairing immediately blocks new remote work
- let established No-Visa connections survive GUI closure, but require credential-store authentication for every new or reconnected session; locked storage yields `waiting_for_auth`, never a headless prompt or cached extractable key
- keep SSH host authentication separate from Jet Pairing, honor the user's SSH configuration and known hosts, and never silently weaken or bypass host-key verification
- establish Pairing with a platform-protected Ed25519 Client identity, one-time short code or 128-bit QR token, target confirmation, and a fresh signed challenge
- promote Workspace changes only after a previewed three-way preflight, preserving conflicts in the isolated Workspace and retaining it until verified success
- persist checkpoint, commit, push, and pull-request work as separate idempotent Effects so partial success is visible and never rolled back or duplicated
- the Rust core and every shipped executable must remain lightweight, start quickly, use little idle memory and CPU, and satisfy enforced per-architecture binary-size budgets
- the Homebrew package checks for required external tools and installs missing formula dependencies; the core itself only detects capabilities and never performs package installation
- let exactly one channel own the desktop core payload and service: GUI-managed immutable versions under `~/.jet`, or a Homebrew-managed prefix and `brew services` with Jet self-update disabled; channel changes are explicit, preserve helpers and state, and activate only after the old service releases ownership
- retain an owner-only structured Security audit for security-sensitive actions and decisions for one year by default, configurable no lower than 90 days, excluding credentials, prompts, terminal output, and file content; Conversation deletion leaves only disclosed opaque target metadata until audit expiry
- integrity-chain the Security audit against a durable head outside rollback-capable SQLite state; on validation failure, preserve reads, export, and existing Runs while blocking new trust, policy, Craft, and destructive mutations until explicit owner recovery starts a recorded new epoch
- v1 has no analytics or remote telemetry; bounded, redacted diagnostic logs and crash reports remain local until the user exports them
- pin v1 Harness parity to a conformance matrix of tested Codex and Claude Code versions, classify each capability as native, Jet-equivalent, generic Presentation fallback, or unavailable because the Harness does not expose it, and warn visibly for unverified newer Harness versions
- notifications: agents already emit completion events, deliver them natively per platform

## v1 scope decisions

- conversations and runs are distinct; conversation view and live view do not determine retention; retain by default, and automatically forget only after no active or pending work, enabled schedule, pin, dirty Workspace, unpushed work, or unresolved Effect remains
- delete and autodelete rules are release blockers
- bring your own provider / model support is exposed through installed harnesses and Jet Crafts
- Codex and Claude Code are the only bundled v1 harnesses
- v1 platforms are macOS, iOS, and Linux; Windows support is deferred

## design

- just steal codex app for macOS.
