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

ADR-0104 pins this matrix to **Codex CLI 0.153.4**, using the non-experimental
schema emitted by `codex app-server generate-json-schema`.

| Capability | Delivery | How |
| --- | --- | --- |
| Start a Conversation | Native | `initialize`, `initialized`, then `thread/start` |
| Exchange turns | Native | `turn/start` on the open app-server input |
| Structured progress and plans | Native | Complete `item/*` and `turn/plan/updated` events |
| Report usage | Native | Complete `thread/tokenUsage/updated` events |
| Interrupt turn | Native | `turn/interrupt`; the app-server remains alive |
| Waiting for approval | Native | Codex server requests answered only through Jet |
| Waiting for authentication or quota | Native | Structured Codex error information |
| Presentation blocks | Jet-equivalent | Bounded inert views derived from native events |
| Stop Run | Jet-equivalent | Signal escalation through `jetfueld` (ADR-0083) |
| File change evidence | Jet-equivalent | Workspace Change checkpoints provide Git object evidence |
| Unknown future native events | Generic fallback | Retained raw JSON with no guessed semantics |
| Oversized native-event Artifacts | Unavailable | Immutable Artifact publication is owned by #47 |
| Harness extension lifecycle | Unavailable | Skills, MCP servers, hooks, and plugins are owned by #37 |

A Codex release other than 0.153.4 may still run, but the Craft writes a visible
unverified-compatibility diagnostic. A schema change never silently expands
the matrix.

## Validation

`just test -p jet-craft-codex` drives a real Craft process and `jetfueld`
against a controlled app-server peer. One native thread exchanges turns,
emits a plan, reasoning, assistant text, usage, and a precision-sensitive
integer; waits for an approval; interrupts a live turn; and exits cleanly. The
test observes only the public Craft contract and the decision delivered to the
native peer.
