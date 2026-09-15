# The Codex Craft

`jet-craft-codex` is the bundled Jet Craft for the Codex Harness (ADR-0009).
The host gives it a private execution connection, and the Craft asks each
Run's own `jetfueld` to launch `codex app-server --stdio`. The Craft owns no
processes, credentials, Project paths, or core state.

## Native protocol authority

The app-server's newline-delimited JSON-RPC protocol is the only semantic
source (ADR-0002). The Craft initializes the server, creates one native thread,
and submits each admitted turn with `turn/start`. It retains every complete
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

## Validation

`just test -p jet-craft-codex` drives a real Craft process and `jetfueld`
against a controlled app-server peer. One native thread exchanges turns,
emits a plan, reasoning, assistant text, usage, and a precision-sensitive
integer; waits for an approval; interrupts a live turn; and exits cleanly. The
test observes only the public Craft contract and the decision delivered to the
native peer.
