# The Claude Code Craft

`jet-craft-claude` is the bundled Jet Craft for the Claude Code Harness (ADR-0009). The host starts it with a private endpoint and serves one execution connection per Run there; the Craft owns no processes, credentials, or core state, and asks each Run's own helper to launch and feed the Harness (ADR-0060).

## The native protocol is the only source

Claude Code is structured throughout, so nothing here reads a terminal (ADR-0002). The Craft launches

```
claude --print --input-format stream-json --output-format stream-json --verbose \
       --permission-prompts host --session-id <run>
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
| Report usage | Native | `result` carries `usage`, `total_cost_usd`, and `modelUsage` |
| Resume a Conversation | Native | `--resume` with the pinned `--session-id` |
| Interrupt turn | Native | `interrupt` control request; the Run stays active |
| Waiting for quota | Native | `rate_limit_event` status |
| Presentation blocks | Jet-equivalent | Views built from native content blocks |
| Stop Run | Jet-equivalent | Signal escalation through the helper (ADR-0083) |
| File change evidence | Jet-equivalent | The Harness reports no object identities; Workspace comparison covers checkpoints |
| Approval requests | Not yet delivered | See below |
| Harness extensions | Unavailable | Skills, MCP servers, and hooks are #37 |

## Approvals are not yet routed

`--permission-prompts host` names the SDK host as the answerer, but a host that only speaks the stream-json message protocol never receives the request: anything that would prompt is denied and reported as `system/permission_denied`. The control protocol's `initialize` does not change this — its request accepts `hooks`, `skills`, `sdkMcpServers`, and `title`, and nothing that routes permissions.

The mechanism that does route them is `--permission-prompt-tool` naming a tool on an SDK MCP server the host serves over `mcp_message` control requests. Implementing that server is the remaining acceptance criterion of [#34](https://github.com/apexgang/jet/issues/34); until it lands, a Run gets exactly the tools its permission mode already allows.

## Validation

`just test -p jet-craft-claude` drives real processes: a host, this Craft, a real `jetfueld`, and a Harness speaking the native protocol. It runs a Conversation of four turns over one process — including one that only ends because it was cancelled natively — and asserts the launch flags, the pinned Conversation identity every completion carries, the turn outcomes, the activity sequence, and that an assistant event's exact bytes and its three views both arrive.
