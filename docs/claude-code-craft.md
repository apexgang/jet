# The Claude Code Craft

`jet-craft-claude` is the bundled Jet Craft for the Claude Code Harness (ADR-0009). The host starts it with a private endpoint and serves one execution connection per Run there; the Craft owns no processes, credentials, or core state, and asks each Run's own helper to launch and feed the Harness (ADR-0060).

## The native protocol is the only source

Claude Code is structured throughout, so nothing here reads a terminal (ADR-0002). The Craft launches

```
claude --print --input-format stream-json --output-format stream-json --verbose \
       --permission-prompt-tool mcp__jet__approve --session-id <run>
```

and both directions are newline-delimited JSON. The process keeps reading its standard input for as long as that input is open, which is what Helper 1.3 exists for (#90): every later turn is a user message written there, and the Harness exits when the input closes.

`--session-id` pins the native Conversation identity to the Run before any output exists, so a resumable identity is never inferred from a race with the Harness's first event. A recovered execution passes that identity back with `--resume`.

The Harness is named by the Craft's own accepted declaration, read from `.jet/craft-spec.toml` beside the executable. The host compares the declaration a handshake carries against the document it accepted, so this read chooses which declaration to present and never what it is allowed to do; the helper independently enforces the executable disclosure it was configured with.

## What the Craft concludes, and what it forwards

Every native event reaches the host whole, including unknown fields and integers no GUI's JSON number type could hold. Only the few discriminators a Run's lifecycle depends on are read: `result` completes the turn in flight and names its native Conversation, `system/init` reports work starting, and a `rate_limit_event` whose status is not `allowed` reports waiting for quota. Everything else stays opaque, which is how a Harness release may add events without this Craft having to know them.

Portable views accompany, never replace, those events (ADR-0008): assistant text becomes Markdown, reasoning becomes text, and a tool call is named rather than described — rendering its arguments generically is how a view stops being inert. Views are bounded so the host's observation budget is spent on the native event rather than on a second copy of it.

Interrupting a turn uses the Harness's own `interrupt` control request, which is the native cancellation Craft 1.4 prefers over signal escalation: the Harness abandons that turn and stays available for the input queued behind it. Stopping the Run remains the escalation ladder in [Execution control](execution-control.md).

## v1 conformance matrix

ADR-0104 pins v1 parity to explicitly tested releases. This Craft is tested against **Claude Code 2.1**. A release outside that range still runs, and reports a visible unverified-compatibility warning on its diagnostics.

| Capability | Delivery | How |
| --- | --- | --- |
| Start a Conversation | Native | `--print` with the stream-json protocol on both sides |
| Exchange turns | Native | User messages written to open standard input (Helper 1.3) |
| Structured progress | Native | `assistant` content blocks retained whole |
| Report usage | Native + normalized | `result` carries `usage`, `total_cost_usd`, and `modelUsage`; the counts and the `rate_limit_event` window are also reported as Usage records |
| Resume a Conversation | Native | `--resume` with the pinned `--session-id` |
| Interrupt turn | Native | `interrupt` control request; the Run stays active |
| Waiting for quota | Native | `rate_limit_event` status |
| Presentation blocks | Jet-equivalent | Views built from native content blocks |
| Stop Run | Jet-equivalent | Signal escalation through the helper (ADR-0083) |
| File change evidence | Jet-equivalent | The Harness reports no object identities; Workspace comparison covers checkpoints |
| Approval requests | Native | The Harness's own permission tool, answered by Jet |
| No-Visa remote tools | Jet-equivalent | Craft 1.6 exposes `mcp__jet__remote` only for an admitted No-Visa Run |
| Harness extensions | Unavailable | General skills, MCP servers, and hooks are #37 |

## Approvals

Before using a tool, Claude Code asks the tool named by `--permission-prompt-tool`. The Craft registers an in-process MCP server for that tool with an `initialize` control request written before the first turn, and the Harness then reaches the server by sending its JSON-RPC messages back out as `mcp_message` requests on the same output stream.

Server plumbing — `initialize`, `tools/list`, and a notification — is answered by the Craft itself, because none of it is a decision. A call of the permission tool is a decision, so the Craft holds it, forwards the native request whole, and reports the Run as waiting for approval. Nothing proceeds until Jet answers with an `approval` action naming that exact request; the Craft then replies to the Harness with `allow` carrying the input it was shown, or `deny`. An allowed call is never edited on the way through: an approval is for what was shown.

The held request travels in the Run's checkpoint, so a Craft that restarts while the Harness waits still delivers the decision instead of leaving it blocked forever.

What does not work, so it is not tried again: `--permission-prompts host` alone never delivers the request — a host that speaks only the stream-json message protocol sees `system/permission_denied` and the call is refused. The control protocol's `initialize` does not route permissions either; its request accepts `hooks`, `skills`, `sdkMcpServers`, and `title`, and nothing else.

## Validation

`just test -p jet-craft-claude` drives real processes: a host, this Craft, a real `jetfueld`, and a Harness speaking the native protocol. It runs a Conversation of five turns over one process — one that only ends because it was cancelled natively, and one that cannot proceed until an approval is answered — and asserts the launch flags, the pinned Conversation identity every completion carries, the turn outcomes, the activity sequence, that an assistant event's exact bytes and its three views both arrive, and that the decision reached the Harness with the input it was shown.

The permission contract itself was verified against Claude Code 2.1.263 directly: registering the server, answering `initialize` and `tools/list`, and allowing one `tools/call` let a real Harness complete a write it would otherwise have been refused.

## No-Visa tools

An accepted Craft 1.6 installation declares `remote_tools`. For a No-Visa Run,
the origin sends the immutable destination selection before Start or Recover.
Only then does this Craft add `remote` to its existing `jet` MCP server. Its
input schema is generated from `CraftRemoteTool`; native input cannot supply
an originating Actor, accepted permissions, or SSH endpoint.

The Craft forwards the operation identity unchanged and delivers Jet's result
as the native MCP response. It holds the corresponding helper source record
until that response has been handled. A restart therefore replays the same
operation identity, allowing the destination's durable receipt to prevent a
second mutation. A pending destination approval is returned as such; the Craft
does not approve or automatically retry it. Native control messages and remote
results remain subject to the usual source and framing bounds.

The conformance suite covers the remote call and response using the real Craft
and helper with a native protocol Harness double. Native remote checkpoints,
extensions, discovery, and sandbox internals remain unavailable as described
in [No-Visa execution](no-visa-execution.md).
