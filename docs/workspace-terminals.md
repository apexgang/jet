# Workspace terminals

Jet protocol 1.16 adds Workspace terminals (issue #24; ADR-0004,
ADR-0038, ADR-0082). `open_terminal` takes a registered `workspace_id`
and initial `rows` and `columns`, each from 1 to 1000. The durable Command
result contains a `terminal` with `terminal_id`, `workspace_id`, and
`state`. `workspace_terminals` returns up to 64 states, live terminals first
and then recent closed history, with a snapshot Event cursor. Lifecycle
changes also produce Events. At most 64 terminals may be live per Workspace.
`close_terminal` takes `terminal_id` and commits an explicit close Effect.

An opening terminal is not ready for input. Once its state is `open`, send
this control on an unused numbered stream:

```json
{"kind":"attach_terminal","id":1,"terminal_id":"<uuid>","after":"0","credit":"65536"}
```

After `terminal_attached`, the same stream carries raw input and output
data frames. `resize_terminal` with a request `id`, `rows`, and `columns`
returns `terminal_resized`. Further output credit uses the existing
`{"type":"credit","bytes":65536}` stream control. Up to 16 terminal streams
may be attached per connection; outstanding credit is bounded to 16 MiB
per stream. Each input or output chunk is at most 64 KiB; output also respects the
client’s negotiated frame limit. Input has no
automatic retry: a lost delivery acknowledgement is `outcome_unknown`.

Each attachment has its own next-byte cursor. Advance it by raw output
length, or by the range in an explicit `terminal_gap` control. Reconnect
with that cursor. The terminal-role helper continuously rolls its eight-MiB
raw spool, including while the daemon or GUI is disconnected. If the
requested range was displaced, a gap precedes the first available bytes.
`terminal_finished` follows the final bytes. Neither disconnect nor attach
launches a replacement shell.

Open and close use the transactional Effect outbox. The helper's private
directory and create-new launch barrier prevent automatic duplicate
launches after uncertain starts. Recovery checks the accepted OS boot,
Workspace and Project roots, helper digest, process start identity, and a
fresh instance-bound socket exchange. An execution proven gone is closed;
uncertain identity or transport is unavailable. Run completion has no
terminal lifecycle effect. Unmatched helpers and unsafe Workspace roots
appear in `orphaned_executions` with `role: "terminal"`; only read-only
metadata is exposed. The existing `resolve_execution` Command accepts
`leave`, `terminate`, or `adopt`, bound to the inspected instance. Adoption
requires matching authoritative records and revalidated roots.

Only authenticated interactive clients may use these operations. Terminal
input audit records identify the client and terminal without retaining
input or output. Raw terminal traffic never enters SQLite, Conversation
history, or the Search index.
