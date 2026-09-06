# Jet Agent Orchestration

Jet coordinates coding harnesses across Planes and presents their work through native and cross-platform GUI clients.

## Language

**Conversation**:
A logical interaction between a user and a harness. It may span multiple runs, and whether Jet retains it after its final run is configurable.
_Avoid_: Chat, session, agent

**Home Plane**:
The single Plane whose `jetd` owns a Conversation's authoritative state. Visa Runs execute there; No-Visa Runs keep their Harness there while operating on paired destination Planes.
_Avoid_: Coordinator, fleet leader, current GUI device

**Plane transfer**:
An explicit change of a Conversation's Home Plane without continuous cross-Plane replication.
_Avoid_: Handoff, No-Visa operation, Workspace transfer

**Prepared transfer**:
A validated but inactive Conversation import that cannot run work or fire schedules until its source has relinquished authority.
_Avoid_: Conversation replica, new Home Plane, restored Conversation

**Transfer tombstone**:
A time-limited source-content record left after a successful Plane transfer for recent recovery and explanation. It is not the permanent authority barrier.
_Avoid_: Authority fence, Conversation copy, archive

**Authority fence**:
A permanent, content-free record that retires a Conversation authority epoch after Plane transfer and prevents restored or stale state from continuing the original authoritative Conversation.
_Avoid_: Transfer tombstone, deletion ledger, Conversation replica

**Project**:
A registered Git repository in which one or more Conversations perform work.
_Avoid_: Workspace, folder, worktree

**Project removal**:
A user-only, two-phase destructive operation that removes a Project from one Plane after showing its exact path and risk state and receiving mandatory typed confirmation. Protected paths and active Projects are rejected; Harness and Jet Craft interfaces cannot invoke it.
_Avoid_: Unregister Project, Workspace cleanup, autodelete

**Project unregistration**:
A non-destructive operation that removes a Project from Jet's registry while leaving its repository and files on the Plane.
_Avoid_: Project removal, delete repository

**Workspace**:
An isolated Git worktree owned by one Conversation so concurrent Conversations can modify the same Project without overwriting one another.
_Avoid_: Project, repository, local checkout

**Workspace promotion**:
A user-directed application of a Workspace's changes to a permanent Project checkout or branch. Conflicts remain isolated in the Workspace rather than overwriting the destination.
_Avoid_: Workspace move, automatic merge, Project transfer

**Workspace seed**:
The Local-checkout changes a Workspace starts with, chosen as none, every eligible change, or named paths, captured as one immutable Git tree and applied over the base as the Workspace is created. A Workspace the seed cannot be applied to is not created.
_Avoid_: Stash, patch, Change checkpoint, snapshot

**Local checkout**:
The user's original working directory for a Project, outside Jet's isolated Workspace lifecycle. Jet permits at most one active managed Run there while external processes remain outside its control.
_Avoid_: Workspace, Project

**Working tree**:
Where a Conversation does its work: a managed Workspace of a Project, the Project's Local checkout, or nowhere yet when it has no Project. It is recorded on the Conversation and chosen when the Conversation is created.
_Avoid_: Placement, checkout mode, worktree

**Conversation view**:
A GUI view organized around conversations, including retained conversations with no active run.
_Avoid_: Codex-style view, persistent view

**Live view**:
A GUI view organized around active runs and managed processes. It does not determine whether their conversations are retained.
_Avoid_: Herdr-style view, ephemeral view

**Retention policy**:
A conversation-level choice controlling whether Jet retains the conversation after its final run exits. Retention is the default; automatic forgetting waits until the Conversation has no active or pending work, enabled schedule, pin, dirty Workspace, unpushed work, or unresolved Effect.
_Avoid_: View mode, persistence mode

**Run**:
A bounded execution of a Conversation on its Home Plane. A Conversation has at most one active Run and may have multiple historical Runs; a No-Visa Run may operate on paired destination Planes without moving authority.
_Avoid_: Process, session, agent

**Run lifecycle**:
The mutually exclusive progression `created`, `starting`, `active`, `stopping`, then `completed`, `failed`, `canceled`, or `lost`.
_Avoid_: Activity, Harness status, process state

**Run activity**:
The current reason an active Run is working or waiting: user input, approval, authentication, quota, or reconnection.
_Avoid_: Lifecycle, terminal status

**Interrupt turn**:
A request to stop the current Harness turn without intentionally ending its Run. Jet preserves partial output and Workspace changes; if forced process termination becomes necessary, the Run ends and the Conversation remains resumable through a later Run.
_Avoid_: Stop Run, cancel Conversation, withdraw queued turn

**Stop Run**:
A request to end the entire active Run through escalating graceful termination when necessary, while preserving its partial output and Workspace changes.
_Avoid_: Interrupt turn, delete Run, forget Conversation

**Conversation pin**:
A pin in a shared or per-client Conversation layout that raises a Conversation's visibility and protects its history and Workspace from automatic cleanup. Runs cannot be pinned.
_Avoid_: Run pin, Workspace retention policy

**Conversation layout**:
The persisted order and pins for Conversations on one Plane. A shared layout is the default; a GUI may disable synchronization and use a private layout associated with its Client identity. Pins in either layout protect cleanup.
_Avoid_: Run order, Plane registry, sidebar cache

**Run order**:
A GUI-local presentation order for Runs. Reordering does not mutate orchestration state and is not synchronized through `jetd`.
_Avoid_: Run priority, execution order, pin

**Turn queue**:
The authoritative sequence of admitted user, schedule, and Auto-continue inputs waiting behind the active turn of one Conversation. User turns are never replaced; schedule and Auto-continue each have one replaceable pending slot.
_Avoid_: Draft, execution priority, interrupt request

**Actor**:
The authenticated origin responsible for a Command or Event, such as an interactive user client, Harness through a Craft, Scheduled task, Auto-continue policy, Utility-model job, or internal recovery operation.
_Avoid_: Provider account, process owner, display name

**Command**:
An authenticated request to change Jet state, carrying an idempotency identity and, for conflict-sensitive changes, an expected revision.
_Avoid_: Event, effect, terminal command

**Revision**:
A monotonic resource version used as a precondition for conflict-sensitive Commands.
_Avoid_: Event sequence, Git commit, protocol version

**Effect**:
A durable request for external work, such as starting a process, issuing a remote operation, or changing Git state, executed only after its initiating transaction commits.
_Avoid_: Command, Event, Presentation block

**Outcome unknown**:
The result of an Effect whose acknowledgement was lost and whose safety cannot be established through reconciliation. Jet never treats it as a definite failure or retries it automatically.
_Avoid_: Failed, unavailable, timed out

**Jet protocol**:
The versioned protocol through which GUI clients control and observe a Plane locally or remotely.
_Avoid_: Craft protocol, Harness protocol, transport socket

**Managed process**:
An operating-system process supervised by Jet as part of a run.
_Avoid_: Agent, conversation

**Harness**:
An external coding-agent system that Jet orchestrates, such as Codex or Claude Code.
_Avoid_: Agent, model, provider

**Provider**:
A vendor that supplies inference models, such as OpenAI or Anthropic.
_Avoid_: Harness, account, model

**Provider account**:
A logical provider identity federated by GUI clients from matching Plane-local Account bindings. No Plane owns or continuously synchronizes it as a global record.
_Avoid_: Provider, Plane, harness

**Account binding**:
A Plane-local authentication binding that allows one Provider account to be used on that Plane. One Provider account may have bindings on several Planes.
_Avoid_: Provider account, credential, login session

**Credential reference**:
An opaque identifier stored by Jet that resolves through the platform credential store or an external authentication helper without persisting secret material under `~/.jet`.
_Avoid_: Credential, token, environment value

**Credential source**:
The backend one Account binding resolves its Credential through: the platform credential store, an explicitly configured external helper, native Harness authentication from the launch environment, or the memory of one daemon start. Each names the limitation it carries, and none of them is a plaintext fallback.
_Avoid_: Credential store, credential helper, secret backend

**Model**:
An inference model made available through a provider account.
_Avoid_: Provider, harness, agent

**Plane**:
A computer that runs `jetd` and can host managed processes.
_Avoid_: Node, machine, host, device

**Setting**:
A mutable Plane value that changes through authenticated Commands and resolves from built-in defaults through the Plane, Project, and Conversation scopes, except where it is restricted to narrower ones. A restriction says where a value may be stored, not what it applies to. Bootstrap values in `~/.jet/config.toml` are not Settings.
_Avoid_: Configuration, preference, option

**Capability snapshot**:
A point-in-time report of a Plane's operating system, core and tool versions, credential availability, installed Crafts, supported Harnesses, and degraded conditions. Commands revalidate required capabilities before acting.
_Avoid_: Fleet inventory, continuously synchronized status, cached authorization

**GUI client**:
A platform-native or cross-platform graphical application that connects to one or more Planes through `jetd`.
_Avoid_: Device, frontend

**Client identity**:
The durable identity of one Jet installation, used by its GUI and desktop `jetd` for Actor attribution, private layout ownership, and paired remote access. Local access is authorized by owner-only IPC; remote use additionally requires Pairing.
_Avoid_: Paired client, user account, Plane identity

**Pairing**:
A one-time, mutually confirmed association that authorizes a GUI client to control a Plane and, from a desktop installation, authorizes its No-Visa Runs to operate there. It starts with a code or QR payload issued by the target Plane; no second agent-access grant exists.
_Avoid_: Plane discovery, fleet enrollment

**Pairing gate**:
A Plane-level switch controlling whether new GUI clients may begin Pairing. It does not alter existing pairings.
_Avoid_: Remote control, client permission

**Paired client**:
A Client identity accepted by a Plane for remote access and independently enabled, disabled, or revoked there. A desktop installation can use it for both GUI Commands and No-Visa tools; a mobile installation has no local Harness and therefore uses it only for GUI Commands.
_Avoid_: Controlling Plane, host, SSH session

**Plane registry**:
The local set of paired Planes known to one GUI client. It is not synchronized between clients.
_Avoid_: Fleet, cluster, Plane database

**jetd**:
The Jet daemon that owns the authoritative orchestration state for one Plane.
_Avoid_: Backbone server, coordinator

**jetfueld**:
A lightweight per-execution helper that keeps either a Harness Run or an open Workspace terminal and its bounded replay history alive while `jetd` is unavailable.
_Avoid_: Daemon, harness, general process supervisor

**Orphaned execution**:
A live `jetfueld` execution whose identity cannot be matched safely to authoritative Plane state after recovery. Only an interactive user may inspect it and explicitly adopt, leave, or terminate it.
_Avoid_: Lost Run, stale descriptor, external process

**Visa mode**:
An execution mode in which the Harness, its Craft, and its `jetfueld` run on the selected destination Plane. It provides the Harness's exact native filesystem, tool, extension, checkpoint, and sandbox behavior there.
_Avoid_: Resident mode, remote-tools mode

**No-Visa mode**:
An execution mode in which the Harness remains on its origin Plane and operates on one or more paired destination Planes through Jet remote tools over SSH. Pairing is the authority to use those tools. Jet aims for equivalent GUI, Git, terminal, file, diff, approval, and audit behavior, but does not claim exact parity for Harness-native checkpoints, tool discovery, extensions, or sandbox internals.
_Avoid_: Plane Tools mode, transparent SSH mode, native remote mode

**Jet Craft**:
An installable harness adapter that teaches Jet to orchestrate one harness. It does not customize the GUI.
_Avoid_: Jet plugin, UI plugin, generic extension

**Craft specification**:
The versioned declaration of a Jet Craft's identity, compatibility, supported Harness features, Jet-enforced broker permissions, expected host access, and distributable Artifacts.
_Avoid_: Plugin manifest, harness configuration

**Presentation block**:
A platform-neutral description of harness content or interaction that a GUI client can render without harness-specific plugin code. It accompanies rather than replaces the harness-native event.
_Avoid_: Normalized harness event, native event

**Harness extension**:
Functionality native to a harness, such as a skill, MCP server, hook, or harness-native plugin, that Jet browses, installs, updates, removes, and presents through the corresponding Jet Craft without converting it to a Jet-specific format.
_Avoid_: Jet Craft, GUI extension

**Extension catalog**:
A searchable inventory supplied by a Jet Craft from Harness-native official sources, configured native marketplaces, or explicit user-added Git URLs. Jet has no universal marketplace in v1.
_Avoid_: Jet marketplace, Craft registry, web search

**External process**:
A harness process started outside Jet management that Jet may detect and inspect for importable native conversation identity.
_Avoid_: External agent, attached conversation

**Imported conversation**:
A harness-native conversation discovered outside Jet and registered so Jet can resume it as a new managed run.
_Avoid_: Attached process, seized process

**Autodelete rule**:
A user-approved structured condition that selects eligible inactive Conversations for Jet Trash. It may be drafted from natural language, but only its reviewed deterministic form executes; after its grace period, explicit authorization may also permit native deletion.
_Avoid_: Retention policy, cleanup preference

**Scheduled task**:
A durable schedule and prompt attached to one Conversation. Each firing submits another turn and starts a new Run when none is active; when work is already active or the Plane is offline, only the newest pending firing is retained.
_Avoid_: Standalone schedule, retry policy, automation run

**Schedule firing**:
A deterministic occurrence identified from its Scheduled task and intended instant in the task's original IANA time zone. A nonexistent local time advances to the next valid instant, while a repeated local time uses its first occurrence and fires once.
_Avoid_: Run, retry, timer tick

**Sound cue**:
A deduplicated semantic attention or completion signal. Its playback is controlled by a local Sound profile, and Event-journal replay never emits it again.
_Avoid_: Notification, audio file, harness event

**Sound profile**:
A device-local collection of imported sounds, per-cue mappings, toggles, and playback targets. If custom playback fails, Jet falls back to the platform's default notification sound while sound effects remain enabled.
_Avoid_: Shared sound library, conversation setting, harness setting

**Git automation policy**:
Independent Project defaults, overridable per Conversation, for automatic branch creation, commits, pushes, and draft pull-request creation. Every operation remains available manually and destructive Git automation is excluded.
_Avoid_: Git mode, agent Git policy, delivery pipeline

**Git message instructions**:
Plane-wide natural-language guidance supplied in the GUI and used by the Utility model when generating commit messages and pull-request titles or bodies.
_Avoid_: Agent prompt, repository instructions, commit template

**Change checkpoint**:
A lightweight, immutable turn-boundary record containing before-and-after commit identities, uncommitted changes, changed-file metadata, and artifact references. It is independent of whether the Harness created a Git commit.
_Avoid_: Commit, snapshot, Run diff

**Change origin**:
An evidence-backed attribution of a Workspace change to the user, Workspace terminal, Harness, or an external or unknown source. Filesystem notifications are hints; Git and content inspection establish the change itself.
_Avoid_: Process guess, filesystem event, commit author

**Auto-continue policy**:
The Account-binding default or Conversation override that controls retry timing, message, and limit after structured rate-limit exhaustion. It never silently changes account, model, or Harness.
_Avoid_: Scheduled task, retry policy, Handoff

**Conversation fork**:
A new Conversation created from a selected Change checkpoint, using a separate Workspace and either a native Harness fork or a provenance-marked context package.
_Avoid_: Handoff, duplicate Run, shared Workspace

**Search index**:
A Plane-local full-text index of human-visible Conversation content and metadata. GUI clients federate searches across connected Planes; v1 does not use embeddings or a central index.
_Avoid_: Event journal, artifact store, cloud search

**Artifact**:
An immutable large payload stored outside SQLite under its SHA-256 content address and referenced transactionally from Jet state.
_Avoid_: Workspace file, diagnostic log, database row

**Recovery snapshot**:
A verified point-in-time copy of authoritative state created before schema migration and at configured recovery points.
_Avoid_: Change checkpoint, Event snapshot, Recovery bundle

**Recovery bundle**:
A portable export of a consistent database snapshot, Artifacts, Craft metadata, and non-secret configuration, optionally including selected Workspace patches or snapshots. It is authenticated and encrypted by default; warned unencrypted export is explicit, and credentials are never included.
_Avoid_: Plane replica, credential backup, Project archive

**Recovered copy**:
A non-authoritative import of historical Conversation content under new Conversation identities with schedules disabled until the user reviews and explicitly enables them.
_Avoid_: Plane transfer, authoritative restore, Conversation replica

**Recovery mode**:
A read-only `jetd` state entered after integrity failure, preserving damaged data and allowing diagnosis, export, or explicit restoration without accepting Runs or mutations.
_Avoid_: Safe mode, empty reset, degraded write mode

**Disk-pressure mode**:
A degraded `jetd` state that preserves reads, export, and eligible cleanup while rejecting new Runs and large writes until free-space reserves recover.
_Avoid_: Recovery mode, automatic Project deletion, unrestricted cache eviction

**Workspace terminal**:
A reconnectable PTY session owned by a Workspace and preserved by a terminal-role `jetfueld` independently from Harness Runs. Its commands affect the Workspace diff and audit trail.
_Avoid_: Managed process, Harness terminal, Run

**Energy policy**:
A per-Plane concurrency budget that prevents excess new background Runs and subagents while power is constrained, without terminating active work.
_Avoid_: Scheduler, process priority, Harness setting

**User edit**:
A direct file change submitted through `jetd`, attributed to the user and included in Workspace diffs and Change checkpoints without being attributed to a Harness.
_Avoid_: Harness edit, review comment, manual patch

**Review submission**:
One structured user turn created from a local draft of file-and-line comments in the GUI.
_Avoid_: User edit, side chat, inline annotation event

**Side-chat view**:
An alternate GUI surface for the current Conversation and Turn queue, carrying structured file or line references without creating a hidden Conversation.
_Avoid_: Conversation fork, secondary agent, utility-model chat

**Jet Trash**:
A recoverable holding area for conversations selected for deletion. Active and pinned conversations are ineligible.
_Avoid_: Archive, permanent deletion

**Utility model**:
The smallest suitable, low-effort model selected through a Plane's Utility Account binding for Jet-owned work such as automatic naming and natural-language rule compilation.
_Avoid_: Default harness model, conversation model

**Utility Account binding**:
The Plane-local Account binding authorized for Utility-model work. Jet never silently substitutes a different Provider, Account binding, or Plane.
_Avoid_: Provider account, Conversation account, fallback account

**Deletion ledger**:
A minimal content-free record of permanently deleted identities and times, retained long enough to prevent an older Recovery snapshot from resurrecting deleted data.
_Avoid_: Event journal, Jet Trash, deleted content archive

**Recovery purge**:
An interactive destructive operation that replaces affected Recovery snapshots with a verified post-deletion recovery point so deleted content no longer remains in Jet's local recovery history.
_Avoid_: Jet Trash purge, snapshot rotation, autodelete

**Path grant**:
An interactive user's explicit authorization for Jet to register or import from a canonical absolute filesystem path. Ordinary Commands instead address files through Project or Workspace identity and validated relative paths.
_Avoid_: Workspace permission, Craft permission, filesystem root

**Automatic review**:
A global-on-one-Plane mode that routes eligible approval requests from every supported Harness to a risk reviewer without widening sandbox or permission boundaries.
_Avoid_: Blanket approval, full access

**Event journal**:
The append-only record of Harness events, client Commands, approval reviews, and lifecycle changes stored by `jetd`, with total ordering guaranteed only within its Plane.
_Avoid_: Security audit, Conversation history, log file

**Diagnostic log**:
A bounded, redacted local record for diagnosing core failures. It is not domain history and never stores credentials, prompt bodies, or terminal output.
_Avoid_: Event journal, audit record, Conversation history

**Security audit**:
An owner-only, structured record of security-relevant actions and decisions, separate from the Event journal. It contains attribution and outcome metadata but excludes credentials, prompts, terminal output, and file content.
_Avoid_: Event journal, Diagnostic log, Conversation history

**Security-degraded mode**:
A restricted state entered when Security-audit integrity cannot be validated. Reads, export, and existing Runs continue, while new trust, policy, extension, and destructive mutations wait for explicit owner recovery.
_Avoid_: Recovery mode, disk-pressure mode, Automatic review

**Handoff**:
A cross-harness continuation that creates a new Conversation under a target Harness from an explicit context package while preserving its source Conversation.
_Avoid_: Workspace transfer, Plane transfer, Conversation fork

**Usage record**:
A provider-reported quota measurement or Jet-observed consumption measurement with explicit source, scope, estimation, and finality metadata.
_Avoid_: Token count, account balance, billing record
