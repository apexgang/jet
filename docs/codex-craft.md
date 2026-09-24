# The Codex Craft

`jet-craft-codex` is the bundled Jet Craft for the Codex Harness (ADR-0009).
The host gives it a private execution connection, and the Craft asks each
Run's own `jetfueld` to launch `codex app-server --stdio`. The Craft owns no
processes, credentials, Project paths, or core state.

## Native protocol authority

The app-server's newline-delimited JSON-RPC protocol is the only semantic
source (ADR-0002). The Craft initializes the server, creates one native thread
or reopens an imported or retained identity with `thread/resume`, and submits
each admitted turn with `turn/start`. It retains every complete
server response, notification, and request as native JSON before deriving any
Run state or Presentation block. It does not parse terminal text. Unknown
native methods remain lossless output with no inferred meaning, so generic
clients can still render or inspect them.

Codex asks for command, file-change, and permission approval with server
requests. The Craft retains the complete request and holds its JSON-RPC ID.
Only an authenticated Jet approval action for that exact ID produces an
`accept` or `decline` response; server plumbing never decides on the user's
behalf. Quota and authentication error codes become Run waiting activities,
while their complete native errors remain available.

Portable views accompany native events (ADR-0008). A completed agent message
is Markdown, reasoning is plain text, and `turn/plan/updated` is a Markdown
checklist. Views are bounded to 4 KiB and never include tool arguments.

## v1 conformance matrix

ADR-0104 pins this Craft to **Codex CLI 0.153.4**, using the non-experimental
schema emitted by `codex app-server generate-json-schema`. The classification
of every v1 capability for that release is published in
[conformance-matrix.md](conformance-matrix.md); a unit test keeps that file
and the pin in `harness.rs` together. A Codex release other than 0.153.4 may
still run, but the Craft writes a visible unverified-compatibility
diagnostic. A schema change never silently expands the matrix.

## Recovery and Model selection

Craft 1.10 reports the resolved Model from the native thread response before
submitting input. A Model-pinned resume passes the requested Model to
`thread/resume` and verifies the returned thread identity and Model. Every
`turn/start` then carries the resolved Model, including Auto-continue turns.
A mismatch closes the connection before any turn is submitted.

After a Craft or daemon restart, `Recover` reconnects the existing helper
without launching another app-server or resending input. The acknowledged
checkpoint retains the native thread, turn, request counter, outstanding
approval, pending JSON bytes, and turn correlation state. Before sending native
input, the Craft syncs a content-free delivery marker beside the helper. If a
crash leaves that input outside the host's committed checkpoint, recovery
refuses to replay it because its outcome is unknown. An acknowledged later
checkpoint clears the marker; a lost acknowledgement can also be reconciled
against the host's committed source offset.

## No-Visa remote tools

The host's `ConfigureRemoteTools` installs a required `jet` MCP server for
this thread through native configuration overrides. Codex starts the bundled
Craft's `--remote-tools-socket` stdio entrypoint. It serves `remote` with the
shared remote-tool schema and the host's selected destinations. The bridge
forwards typed `CraftRemoteTool` calls and returns the broker's outcome;
Pairing, permissions, destination review, and operation deduplication remain
in `jetd`.

The bridge socket sits beside the helper in its owner-only execution
directory. Requests are limited to 1 MiB and served one at a time. Each MCP
request opens a new socket connection, so later calls can reconnect after
Craft recovery. Losing a call's response reports an unknown outcome and
never replays the call automatically. The bridge exists only for Runs
admitted with remote tools.

## Validation

`just test -p jet-craft-codex` drives a real Craft process and `jetfueld`
against a controlled app-server peer. One native thread exchanges turns,
emits a plan, reasoning, assistant text, usage, and a precision-sensitive
integer; waits for an approval; interrupts a live turn; and exits cleanly. The
test observes only the public Craft contract and the decision delivered to the
native peer. Additional cases cover imported-thread resume, rejection of a
changed Model, Craft and host reconnection to the living helper, MCP tool
forwarding, and an interrupted bridge call without automatic replay.
