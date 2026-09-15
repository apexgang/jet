# v1 Harness conformance matrix

ADR-0104 pins v1 Harness parity to explicitly tested releases. This is the
published matrix issue #58 requires: one row per v1 capability a Harness
takes part in, classified for each bundled Craft at its pinned release.
Capabilities that never touch a Harness, such as Plane pairing, schedules,
the Plane-local search index, sidebar layouts, and Plane transfer, are
outside the matrix; they behave the same whichever Harness a Run uses.

## Pinned releases

| Harness | Tested release | Craft protocol | Where the pin lives |
| --- | --- | --- | --- |
| Codex | Codex CLI 0.153.4, exact | Craft 1.8 | `jet-craft-codex/src/execution/harness.rs` |
| Claude Code | Claude Code 2.1.x, by minor line | Craft 1.10 | `jet-craft-claude/src/execution/harness.rs` |

A unit test in each Craft reads this file and fails when the pin it
enforces is not the one written here, so the matrix and the warning it
backs cannot drift apart. A release outside its pin still runs, and the
Craft writes a visible unverified-compatibility diagnostic; a newer Harness
release never silently widens a row below.

## Classes

- **Native**: the Harness delivers the capability through its own protocol
  and Jet delegates to it.
- **Jet-equivalent**: Jet delivers the behavior itself, through the Run's
  own Provider and Account binding or through Workspace evidence, because
  the Harness exposes no equivalent Jet can drive.
- **Generic fallback**: the Harness emits something Jet cannot interpret
  precisely, so it is retained losslessly and rendered as an opaque
  Presentation block.
- **Unavailable**: the Harness, at the pinned release and through the
  bundled Craft, does not expose the capability. ADR-0104 makes every
  feature-list item a release blocker wherever the Harness supports it, so
  each Unavailable row names why it is not a blocker.

## Matrix

| Capability | Codex 0.153.4 | Claude Code 2.1 | Notes |
| --- | --- | --- | --- |
| Start a managed Conversation | Native | Native | `thread/start` over the app-server; `--print` with stream-json on both sides |
| Exchange turns on a live Run | Native | Native | `turn/start`; user messages on open standard input (Helper 1.3) |
| Structured progress | Native | Native | Complete `item/*` events; `assistant` content blocks retained whole |
| Plan status | Native | Generic fallback | `turn/plan/updated`; Claude Code has no plan event, so its plan tool calls render as ordinary content |
| Usage per turn and rate-limit windows | Native, normalized | Native, normalized | `thread/tokenUsage/updated`; `result` and `rate_limit_event`. Both are also Usage records ([usage-records.md](usage-records.md)) |
| Waiting for quota | Native | Native | Structured Codex error; `rate_limit_event` |
| Waiting for authentication | Native | Generic fallback | Codex reports `unauthorized`; Claude Code reports authentication failure only as result text |
| Interrupt turn | Native | Native | `turn/interrupt`; `interrupt` control request. The Run stays alive |
| Stop Run | Jet-equivalent | Jet-equivalent | Signal escalation through `jetfueld` (ADR-0083) |
| Approval requests | Native | Native | Server requests answered only through Jet; the `--permission-prompt-tool` MCP tool Jet serves |
| Automatic review | Jet-equivalent | Jet-equivalent | Neither release exposes a separate native reviewer that satisfies ADR-0012, so the Provider's low-effort reviewer answers and is recorded ([automatic-review.md](automatic-review.md)) |
| Resume a native Conversation across Craft or daemon restart | Unavailable | Native | Claude Code resumes with `--resume` and the pinned session; the Codex Craft declares no `resume` feature, so a lost Run continues as a new Run with a new thread. Not a blocker: the Conversation and its Jet history continue |
| Auto-continue after a rate limit | Jet-equivalent, live connection only | Jet-equivalent | The Codex Craft retries only on its matching live connection; the Claude Craft also opens a new Run through native resume with the observed Model ([auto-continue.md](auto-continue.md)) |
| Model selection and Model-pinned resume | Unavailable | Native | Craft 1.10 explicit native Model selection is implemented by the Claude Craft only. Not a blocker: Codex Runs use the Harness's configured Model |
| Fork a Conversation from a checkpoint | Jet-equivalent | Jet-equivalent | Neither Craft declares `fork`; the new Run starts a native Conversation with the provenance-marked history prefix ([conversation-forks.md](conversation-forks.md)) |
| Handoff to another Harness | Jet-equivalent | Jet-equivalent | Jet composes the Handoff summary; no Harness exports one ([handoffs.md](handoffs.md)) |
| Import an external native Conversation | Jet-equivalent | Jet-equivalent | Identities are discovered and recorded as metadata; a managed Resume makes a Conversation in a registered Project and starts a new Run |
| Current and final diffs, changed files | Jet-equivalent | Jet-equivalent | Workspace Change checkpoints provide Git object evidence; Claude Code reports no object identities ([change-checkpoints.md](change-checkpoints.md)) |
| Presentation blocks | Jet-equivalent | Jet-equivalent | Bounded inert views derived from native events |
| Unknown future native events | Generic fallback | Generic fallback | Retained raw JSON with no guessed semantics |
| Subagents view | Native | Native | Native child activity is visible through native events |
| Subagent concurrency limits | Unavailable | Unavailable | Both Crafts report `monitor_only`; Jet reserves Run slots instead ([resource-budgets.md](resource-budgets.md)). Not a blocker: ADR-0040 accepts native limits only where available |
| Skills, MCP servers, and hooks | Native | Native | Native configuration files edited in their own formats ([harness-extensions.md](harness-extensions.md)) |
| Plugins and marketplaces | Native | Native | Codex plugin RPCs on the app-server; Claude marketplaces and plugin refresh |
| Visa Runs on a destination Plane | Native | Native | Both Provider mappings are supported ([visa-runs.md](visa-runs.md)) |
| No-Visa remote tools | Unavailable | Jet-equivalent | Claude Code reaches `mcp__jet__remote` through its native MCP bridge; Codex has no comparable tool bridge in the bundled Craft ([no-visa-execution.md](no-visa-execution.md)). Not a blocker: a Codex Conversation runs in Visa mode |
| Utility work: names, commit and pull-request text | Jet-equivalent | Jet-equivalent | Both declare `utility`; the Home Plane's Utility binding selects the Model ([utility-work.md](utility-work.md)) |
| Completion notifications | Native | Native | Native completion events reach the GUI's platform notifications |
| Oversized native output | Jet-equivalent | Jet-equivalent | Immutable Artifact publication and binary transfer ([artifacts.md](artifacts.md)) |

## Keeping the matrix honest

- Change a pin only together with a run of that Craft's conformance suite
  (`just test -p jet-craft-codex`, `just test -p jet-craft-claude`) and the
  affected rows.
- A row moves from Unavailable only when the Craft implements it; a new
  Harness release never moves a row by itself.
- The per-Craft documents, [codex-craft.md](codex-craft.md) and
  [claude-code-craft.md](claude-code-craft.md), describe the native
  protocols and defer to this file for classification.
