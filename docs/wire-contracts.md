# Jet wire contracts

`jet-protocol` is the single source of both wire contracts. `just contracts`
regenerates every machine-readable artifact from the Rust DTOs:

| Artifact | Contract |
| --- | --- |
| `packages/jet-protocol/contracts/jet-v1.schema.json` | Client protocol, GUI to `jetd` |
| `packages/jet-protocol/contracts/remote-tool.schema.json` | Native MCP input for the bounded Jet remote tool |
| `packages/jet-protocol/contracts/craft-v1.schema.json` | Craft protocol, `jetd` to Craft |
| `apps/jet-tauri/src/lib/protocol/JetModels.ts` | Client models the Tauri GUI compiles |
| `apps/jet/jet/Protocol/JetModels.swift` | Client models the Swift GUI compiles |
| `packages/jet-protocol/contracts/CraftModels.{ts,swift}` | Craft models for adapter authors |

`just contracts-check` emits the same artifacts into a scratch directory and
diffs them against the committed ones, so a wire change that skips
regeneration fails instead of leaving the GUIs on a stale contract. Schema
emission is an optional `schema` feature; product builds link no `schemars`.

The declarations are models, not permissive JSON decoders: validate the
original frame against the schema, and populate `RawJSON` from original
fragments before parsing numbers (ADR-0089, ADR-0049).

## Codecs serde hides

`serde(with)` changes a field's wire form without changing its Rust type, and
schemars cannot see through it. Sequences, cursors, and Revisions cross as
canonical decimal strings, and fixed-width byte strings as lowercase
hexadecimal, so those fields also name a schema stand-in: `crate::Decimal`,
`crate::OptionalDecimal`, or `crate::Hex<N>`. A new `serde(with)` field
without one publishes the Rust type instead of the wire form.

## Shared corpora

`craft-fixtures.json` and `jet-fixtures.json` pair a `$defs` name with a
payload and the decision every implementation must reach. The Rust decoder
checks them in `just test -p jet-protocol`; `just contracts-test` (Node 24+
and Swift) checks the same payloads against the emitted schema. Between them
they cover optional fields an older reader ignores, rejected unknown message
kinds, duplicate discriminators a dictionary would discard, non-canonical
decimal strings, and hexadecimal of the wrong width or case (ADR-0094).

## Usage negotiation

Jet 1.32 adds `authorize_approval_retry` and `approval_retry_authorized`.
The Command identifies the Run and a denied review, with no replacement
action. Older peers cannot admit it. See [Automatic review](automatic-review.md).

Jet 1.29 adds the `usage` Query and its Plane-local snapshot, and Craft
1.7 adds the `usage` event. Neither needs a new Craft feature or broker
permission: a Usage report grants nothing and changes no Run state. See
[Usage records](usage-records.md).

## No-Visa negotiation

Jet 1.26 adds `start_no_visa_run`, `remote_tool`, exact remote-action review,
and the execution's `no_visa` selection and capability report. Older peers
cannot admit these operations; snapshots omit the new field for older minors.

Craft 1.6 adds `configure_remote_tools`, the `remote_tool` event, and the
`remote_tool_result` command. The accepted Craft must declare the
`remote_tools` feature and broker permission. Configuration precedes Start
or Recover and is immutable for that Run. A Craft receives results for its
own operation IDs; transport failure is an affected-call failure, never an
instruction to replay a mutation under another identity.
