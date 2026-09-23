# Freeze independent desktop client foundations

The native Swift app and the Tauri app are independent peer GUI clients. They share Jet protocol contracts, domain vocabulary, and the presentation fixture corpus, but they do not share a runtime bridge or UI framework. The Swift app connects to `jetd` through its native transport. The Tauri Rust shell connects to `jetd` through `packages/jet-client`. Neither client calls or links the other, and `jetd` remains the only domain authority.

This decision freezes the Wave 0 foundation below. A later change to these boundaries requires an ADR that supersedes this one.

## Adapter interfaces

Views receive typed presentation values and emit typed intents. They do not construct wire envelopes, call a generic Query or Command function, select protocol versions, or handle native transport errors.

The native adapter in each client owns four protocol operations:

1. Read a bounded, typed Plane snapshot.
2. Read or stream ordered Events strictly after a caller-supplied Plane cursor.
3. Send a typed durable Command with a caller-held stable Command ID and a byte-equivalent body across retries.
4. Stop a subscription or connection without implying that daemon work was stopped or rolled back.

The Swift boundary is `JetClient`. Its Wave 0 proof surface is `status()`, `events(after:)`, `eventStream(after:)`, `clearSetting(_:scope:commandID:)`, connection state, and explicit disconnect. Feature-specific adapters may wrap or extend this surface, but feature code must not bypass it.

The Tauri presentation boundary is typed modules under `src/lib/jet`. Direct Tauri `invoke` and `Channel` use stays inside those modules. Rust commands validate arguments, translate them to typed `jet-client` operations, and return bounded presentation DTOs. `openPlaneFeed` is the Wave 0 read-only proof. Later feature commands must be additive and feature-specific; the webview will not receive a generic Query, Command, filesystem, or shell bridge.

Wire integers that can exceed JavaScript's safe integer range cross Tauri IPC as canonical decimal strings. Credentials, private keys, connection proofs, socket paths, raw command bodies, native errors, and unrestricted Event payloads never cross into the webview.

## Connection state machine

Both clients use the same observable lifecycle even when their platform types use different names:

| State | Meaning | Required behavior |
| --- | --- | --- |
| Disconnected | No authenticated transport exists and no adapter retry is active. | Keep the last trusted presentation state visibly stale and disable Commands. |
| Connecting | A transport and fresh handshake are in progress. | Accept no Plane data until negotiation and validation complete. |
| Connected | A fresh handshake completed. | Apply a fenced snapshot, then request Events strictly after its cursor. Tauri may label this state `online`. |
| Reconnecting | A retryable transport loss followed trusted state. | Preserve the last complete cursor, disable Commands, and retry with bounded backoff. |
| Failed | A non-retryable authentication, compatibility, validation, or protocol failure occurred. | Stop automatic retry and expose a stable recovery path or explicit retry. |

The allowed progression is disconnected to connecting; connecting to connected, disconnected, or failed; connected to reconnecting; and reconnecting to connected, disconnected, or failed. Cancellation closes only the owned request, subscription, or connection. It does not mutate authoritative Plane state.

A bounded request returns `offline` after its retry budget is exhausted. A long-lived feed may begin another bounded retry cycle while its owner remains alive. After reconnect, Events resume strictly after the last Event delivered to presentation state. Duplicate or out-of-order Events are rejected before they reach a view.

`cursor_expired` and `cursor_ahead` are snapshot recovery conditions, not transport failures. The root session fetches a new fenced snapshot, replaces derived state, and subscribes after the new cursor. `pagination_stale` discards the current page chain and restarts that read.

## Error taxonomy

Validated server failures preserve their protocol category, stable code, retryability, and structured recovery meaning. The v1 protocol categories are `invalid_input`, `unauthorized`, `conflict`, `unavailable`, `incompatible`, `rate_limited`, `not_found`, `outcome_unknown`, and `internal`. Presentation behavior branches on structured fields, never message text.

Client-generated failures use these stable values:

| Category | Stable code | Meaning |
| --- | --- | --- |
| `offline` | `transport.offline` | The native adapter cannot reach the Plane. It is retryable. |
| `invalid_response` | `protocol.invalid_response` or a narrower `protocol.*` code | The response failed schema, framing, ordering, or semantic validation. It is not retried blindly. |
| `overloaded` | `transport.too_many_requests` | A local bounded-work limit was reached. It is retryable after pressure drops. |
| `cancelled` | `request.cancelled` | The caller cancelled local waiting. It says nothing about daemon rollback. |
| `outcome_unknown` | Server code or the typed Command uncertainty result | A durable Command may have reached `jetd`, but its result was lost. |

When a durable Command becomes uncertain, the client retains its Command ID. It may inspect the result or retry only with the same ID and byte-equivalent body. It never reports success, failure, or rollback without authoritative evidence.

Native operating-system, network, Rust, JSON, and decoder strings stay behind the adapter. User-facing copy may be platform-native, but category, code, retryability, and recovery semantics must remain equivalent. The webview receives only allowlisted, bounded error data.

## Fixture format

`fixtures/desktop/presentation-v1.json` is the single canonical desktop presentation corpus. Format version 1 contains exactly one scenario for each of these states: `first_launch`, `ready`, `active`, `queued`, `approval`, `completed`, `offline`, `stale_cursor`, `denied`, `unsupported`, and `recovery`.

Both clients consume that file without maintaining a client-specific copy. Loaders reject unsupported format or protocol versions, missing or duplicate state coverage, duplicate identifiers or contract references, non-canonical decimal cursors, invalid UUIDs, queues over 128 items, non-contiguous queue positions, and impossible Run activity combinations. Each scenario names the Queries, Commands, stable error categories, restart behavior, or backend dependency that produces it.

The corpus is presentation data only. It cannot authorize a Command, replace a Plane snapshot, imply that an unsupported action succeeded, or contain credentials, real prompts, terminal output, private paths, or production identifiers.

An incompatible structural or semantic change increments `format_version` and ships with both loaders. Additive optional metadata may remain in version 1 only when both loaders safely ignore or validate it and all version 1 invariants still hold. A protocol minor change updates `protocol_version` and both consumers in the same change.

## Client-state ownership

| Owner | State it owns | State it must not own |
| --- | --- | --- |
| `jetd` | Projects, Conversations, Runs, Turns, settings, capabilities, revisions, Command deduplication results, Event journal, and all domain policy | Client window, focus, or draft state |
| Native adapter | Connection and negotiation, request routing, validated decoding, per-Plane cursor progress, reconnect backoff, pending replies, and Command uncertainty | Optimistic domain truth or product policy |
| Root session | Last trusted snapshot, derived presentation models, stale and offline markers, pagination chains, selection, drafts, panel state, and scroll restoration | Authoritative revisions, capabilities, or Command results |
| Client-local native persistence | Stable client identity, window or scene restoration, and non-sensitive appearance, notification, and last-selection preferences | Jet content, credentials, private keys, or connection proofs in browser storage |
| View or webview | Ephemeral control state and typed user intents | Protocol envelopes, secrets, cursors, retry policy, or domain authority |

The Swift app creates one main-actor desktop session per scene. The Tauri app creates one root session controller with feature-scoped Svelte state. Either session can be discarded and rebuilt from a fenced snapshot plus ordered Events. Cached presentation data is always labeled by freshness and never overrides `jetd`.

## Rejected options

- A shared Rust bridge between the two desktop apps would couple independent clients and make platform lifecycle, packaging, and recovery failures shared.
- A generic Tauri Query or Command bridge would move protocol authority and attack surface into the webview.
- Optimistic client-owned domain state would conflict with revisions, duplicate Commands, reconnects, and multi-client use.
- Per-client fixture copies would permit semantic drift before parity tests could detect it.

## Consequences

Wave 1 feature adapters must fit this boundary and add typed operations rather than widen it generically. Remote SSH, Keychain or secret-service storage, and feature Commands extend the native adapters without exposing secrets to views. Every vertical slice must test Query, Command, stream, reconnect, cursor resume, uncertainty, and shared fixtures at the layer it changes. macOS remains the interaction reference, while Linux is held to the same protocol and recovery semantics.
