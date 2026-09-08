# Handoff

Client protocol 1.24 adds `handoff_conversation` (ADR-0021). The caller selects
an existing managed source Run, an installed Craft for a different Harness,
a summary, a plan, and relative file paths. Empty summary/plan strings select
none. The command always captures the current uncommitted diff against HEAD,
including tracked changes and non-ignored untracked files. It neither generates
summaries nor copies Conversation history, credentials, settings, or native
identities. Source Conversation state, files, index, and Runs remain unchanged.

The package contains exactly `summary`, `plan`, `files` (path/content), `diff`,
and `provenance`. Core-observed provenance records the source Conversation and
Run, both accepted Harness identities, HEAD, and the captured tree. Selected
files are regular UTF-8 blobs read from that immutable tree; directories,
symlinks, submodules, ignored untracked files, missing files, and paths outside
the registered root are refused. Symlinks and submodule Git links in the diff
retain existing Workspace semantics without copying their targets.

Limits are UTF-8 bytes: 8 KiB each for summary, plan, and file; 16 distinct files;
16 KiB for the complete diff; 48 KiB for the JSON package after escaping.
Oversized or incomplete captures and non-UTF-8 text diffs are refused without
truncation or byte replacement. Git binary patches remain supported. Existing
capture time/Artifact budgets apply, and initial input fits the 64 KiB Run limit.

One transaction records a new Conversation with inherited retention, a separate
managed Workspace in the same Project at the captured HEAD/tree, its first Run,
and the launch Effect. The independently pinned destination Craft receives a
JSON package inside a `jet-handoff-context` data envelope with angle brackets
escaped, plus a fixed continuation instruction. Native resume/fork identities
are absent. The destination's launch plan and initial Turn retain the package;
execution and recovery never reread the source. Later Runs use their own history.

The response is `conversation_created` with origin `new`. A distinct
`conversation.handoff_created` Event records provenance on the destination;
the source receives no mutation Event. Handoff neither selects a historical
checkpoint, promotes changes, nor transfers Home Plane authority. Byte-equivalent
retries under the same authenticated Command identity return the original
destination. Older minors cannot issue the command and treat its Event as opaque.
Automatic rate-limit policy selection remains a separate caller.

Validation follows ASVS 2.2.1/2.2.2 and 5.3.2; closed package types follow 1.5.2;
Git uses argument arrays (1.2.5); destination state and launch Effect commit
together (2.3.3). Workspace materialization follows the existing synchronous
Workspace contract: a database commit failure can leave an unregistered worktree
(see `packages/jet-core/src/workspace.rs`). Admission failures after Workspace
creation remove that new worktree before rolling back.
