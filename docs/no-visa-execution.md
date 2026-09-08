# No-Visa execution

`start_no_visa_run` starts the Harness, Craft, and helper on the Conversation's
Home Plane. The request selects an origin-local native Account binding and one
to eight destination Plane/Workspace/SSH triples. Changing that selection starts
a new Run. Conversation authority and credentials remain on the origin.

A locally connected desktop installation can admit this mode. The origin daemon
owns direct system SSH connections, signs each fresh Jet challenge with the
installation identity, and forwards a closed remote-tool request. The GUI is not
a relay. A remote connection, including a mobile client, cannot originate this
mode. The destination checks its own Plane identity, live Pairing, registered
Workspace and Project, and the accepted `remote_tools` permission on every call.
There is no second agent-access grant.

## Installation identity and Craft support

The desktop supplies `jetd serve --identity-client-id UUID --identity-signer
/absolute/path/to/helper`. Both options are required together. The configured
helper receives `sign-connection-v1 UUID` as arguments and the bounded Jet
connection transcript on stdin. It resolves the private key through platform
credential storage and writes exactly 64 Ed25519 signature bytes to stdout.
Jet never requests, exports, or persists the private key. The signer runs only
for this installation's locally admitted Runs and has a bounded lifetime.
Missing configuration or a mismatched installation identity refuses admission.
This is a desktop integration interface, not a key-file fallback.

The accepted Craft must support Craft 1.6 and declare the `remote_tools` feature
and broker permission. The bundled Claude Code Craft exposes `mcp__jet__remote`
through its existing native MCP bridge. The bundled Codex Craft currently lacks
that bridge and refuses No-Visa admission. Existing Visa behavior is unchanged.

## Destination tools and limits

| Action | Destination behavior |
| --- | --- |
| `read_file` | Up to 64 KiB of UTF-8 file content under the selected Workspace |
| `write_file` | Atomic replacement of up to 64 KiB; Git metadata writes rejected |
| `git` | Fixed `status` or working-tree `diff`, with external diff, text conversion, hooks, and fsmonitor disabled |
| `process` | Literal executable/argument array, relative cwd, explicit environment changes, exact review |
| `shell` | Separately labelled shell source, relative cwd, explicit environment changes, exact review |
| `terminal` | Real PTY with fixed dimensions and a reviewed input batch, closed after that batch |

Paths reject absolute forms, traversal, invalid components, and symlinks resolving
outside the registered root. Cwd is resolved again immediately before execution.
Process and shell invocations start with a minimal environment. Arguments, shell
source, and terminal input are limited to 16 KiB; environment changes to 32 names
and 8 KiB. Output streams are bounded to 64 KiB each. The destination admits at
most 32 concurrent calls. Each worker has sixty seconds before graceful and
forced stop; the origin bounds the entire SSH call to eighty seconds.

Root validation constrains Jet's direct filesystem access and process launch
location. Reviewed programs still run under the destination user's OS permissions
and any OS sandbox already present; Jet does not manufacture a native Harness
sandbox or claim to confine arbitrary program behavior merely by setting cwd.

The execution capability report identifies files, Git, processes, and bounded
terminals as Jet equivalents. Native remote checkpoints, tool discovery,
extensions, sandbox internals, and persistent terminal sessions are unavailable.
The PTY result merges terminal stdout/stderr, as a real terminal does.

## Review, retries, and revocation

The first process, shell, or terminal request returns `approval_required` without
executing. An authorized destination user queries `remote_tool_review` using the
paired Client ID and operation ID, then submits `review_remote_tool` with
`allow_once` or `deny`. The immutable reviewed request includes the origin,
Workspace, literal arguments or shell/terminal input, and environment changes.
Approvals expire after ten minutes and authorize only that exact operation.

The origin derives provenance and permissions from its admitted Run and accepted
Craft; neither field comes from the Harness tool arguments. Each authenticated
installation/operation pair reserves a digest before external work. An exact retry
returns the stored result, even after restart; changed content is refused. An
interrupted operation without a confirmed result reports uncertainty instead of
running again. No operation is retried automatically. Loss of one destination
fails its affected call while the origin Run can continue with another destination.

Disable, revoke, and replacement Pairing atomically invalidate pending approvals
and stop new admission. In-flight workers receive TERM, then KILL after two
seconds, with up to one additional second to reap. PTY children receive one second
of grace inside that outer bound. Unrelated Visa Runs keep their own supervision;
Conversation ownership is not changed by revocation.

Security audit entries reference the installation/operation pair. Durable operation
metadata retains the origin Plane, Conversation, Run, and destination Workspace.
Audit entries omit input, content, and output. Operational payloads expire after
thirty days; unused reviews are discarded after ten minutes when subsequent work
runs the expiry sweep. Identity tombstones prevent a late retry from duplicating
an old mutation. This follows ASVS 1.2.5, 2.2.1–2.2.2, 8.2.1–8.3.3, 13.2.2,
13.2.6, and 16.2.1–16.2.5.

## Review stages

The coherent implementation spans destination admission and durable receipts,
origin Run selection and SSH brokerage, and the native Craft bridge. Those stages
share the negotiated wire vocabulary and must ship together to expose a usable
mode. Generated Rust-derived Swift/TypeScript/schema updates are mechanical.
The destination API is the smallest independently testable stage; the origin and
Craft stages are covered at their separate process boundaries.
