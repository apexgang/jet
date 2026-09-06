# Jet Craft protocol v1

A Craft is an executable Harness adapter. Rust adapters can use `jet-craft-sdk`; other languages implement the same byte contract. The SDK has no dependency on `jet-core`, creates no processes, and grants no credentials or broker authority. Bundled Harness implementations and execution supervision are separate work in issues #20 and #21.

The wire DTOs live in `jet-protocol`, separately from core domain types. `just craft-contracts` emits `packages/jet-protocol/contracts/craft-v1.schema.json` directly from those DTOs and generates Swift and TypeScript model declarations alongside it. This optional development feature is not linked into normal product builds. The declarations are models, not permissive JSON decoders: validate against the schema, and populate `RawJSON` from original fragments before parsing numbers. `fixtures.json` is shared by the Rust decoder tests and `just craft-contract-test` (Node 24+ and Swift). The subprocess tests exercise the executable SDK contract. `packages/jet-craft-sdk/tests/fixtures/craft-spec.toml` is a complete specification example.

## Transport and handshake

The host maintains one accepted Craft process per artifact digest (ADR-0018), with a separate owner-only byte stream for each execution. At process bootstrap the host provisions a private socket endpoint; the Craft accepts execution connections there and runs one `CraftConnection::accept` per connection concurrently. Socket connection identity routes the execution; no global current Run or shared stdin reader may mix their Commands. The host closes an execution connection independently of other Runs and manages process exit after the idle period (ADR-0058). Private stdin/stdout pipes are also usable for a single connection in adapter development, but are not the multiplexed process transport. Diagnostics belong on stderr.

The SDK's `CraftConnection::accept` takes an async reader, writer, and parsed specification. Use `parse_specification` on bounded `.jet/craft-spec.toml` contents. Parsing performs no filesystem I/O and accepts at most 64 KiB. After negotiation, `split()` returns independent `CraftReceiver` and `CraftSender` halves so native events can be forwarded while a Command receive remains pending.

1. The host writes the ten UTF-8 bytes `jet-craft\n`.
2. The host sends `CraftHello` as a legacy frame: kind byte `0`, big-endian unsigned 32-bit payload length, UTF-8 JSON payload.
3. The Craft responds with `CraftReady`, or `{"kind":"rejected","code":"incompatible"}` and closes. Other stable rejection codes are `invalid_message`, `disconnected`, and `timeout`. Invalid prefaces and handshake timeouts may close without a reply.
4. After Ready, both sides switch to the existing nine-byte frame header: kind byte, big-endian unsigned 32-bit stream ID, big-endian unsigned 32-bit payload length. Craft v1 uses only control frames on stream zero. Binary and numbered streams are rejected; Artifacts are referenced through separately authorized broker operations.

Every control payload is limited to 1 MiB, 64 nested containers, 4,096 direct entries per collection, and 8,192 entries in total. These are the shared Jet codec bounds, not a second parser policy. Startup and writes have ten-second deadlines. Reads may wait while an execution is idle. The SDK never queues output: adapters await each send, providing bounded backpressure without competing binary traffic. Close the connection after any error or canceled receive; partial frames cannot safely be retried.

Hello has this shape (unknown optional object fields are ignored):

```json
{"protocol":{"family":"craft","versions":[{"major":1,"minor":0}],"capabilities":["actions","resume"]},"specification":{"family":"specification","versions":[{"major":1,"minor":0}]},"execution_id":"01900000-0000-7000-8000-000000000001","resume":null}
```

Ready contains `protocol` (the negotiated family, singular `version`, and capability intersection), independently negotiated `specification_protocol`, `specification` (the complete accepted declaration), and `enabled_features`. A newer specification minor containing only optional additions remains compatible with an older reader; it is not an execution pin. The host must compare declarations with the installed, user-accepted specification before sending any Command. A peer's self-description is never an authorization grant.

## Compatibility and restart

`ProtocolOffer` lists one highest minor per supported major. Major zero, duplicate majors, empty offers, family mismatches, and disjoint versions are rejected. New executions select the newest common major and the smaller minor; capabilities are the sorted, deduplicated intersection. `client`, `craft`, `helper`, and `specification` offers negotiate independently. A newer GUI protocol cannot upgrade a running Craft or helper.

The concrete SDK currently implements Craft 1.0 and specification 1.0. There is no previous released Craft major yet; the negotiation conformance test exercises current/previous-major offers with different minor ceilings. Adding a future codec major requires an implementation as well as a declaration. Advertising a new major in TOML cannot make this SDK speak it.

Before acknowledging startup, the host durably records the selected Craft/helper versions with the execution identity. On Craft restart it sends `resume: {"version":{"major":1,"minor":0},"native_conversation":"native-42"}` with the same `execution_id`. The SDK requires that exact saved version from the host, specification, and SDK, plus the `resume` feature and capability. It never chooses a newer major during recovery. Helper recovery uses the same `Negotiation::Resume` contract independently. An updater must use these saved pins to keep an incompatible update staged until executions finish or the user explicitly stops them; download alone does not change the running contract (ADR-0019).

A closed pipe is not evidence that a Command failed. The host reconciles unresolved Effects; the SDK never replays Commands or starts a Harness automatically. The subprocess test terminates a Craft after receiving a completion, starts a new process with the saved native identity and version, and also verifies incompatible recovery is rejected before native output. Persistent supervision and reconciliation are owned by the host, not the adapter SDK.

## Native events, presentation, and actions

Commands use a closed `kind` discriminator:

- `{"kind":"turn","id":"turn-1","text":"Inspect the changes"}` submits an admitted turn.
- `{"kind":"action","id":"action-1","action":{"kind":"invoke","action_id":"inspect","input":{"path":"src"}}}` invokes a presented native action.
- An approval action has `kind: "approval"`, `request_id`, and `decision: "allow_once" | "deny"`. Unknown action kinds and decisions are rejected. The host authenticates and authorizes the exact request before dispatch; Craft declarations never bypass approval policy.
- `{"kind":"shutdown"}` releases this execution's adapter connection without deleting a Conversation or closing other execution connections. The host controls process shutdown after the Craft becomes idle.

`turn` needs the declared `turns` feature. Actions need the declared `actions` feature and negotiated `actions` capability. Shutdown always remains available.

Output carries the entire original native JSON event and zero or more portable views:

```json
{"kind":"output","native_event":{"vendor_event":"delta","vendor_field":9007199254740993},"presentation":[{"kind":"text","text":"Hello"},{"kind":"actions","actions":[{"id":"inspect","label":"Inspect file"}]}]}
```

Known Presentation kinds are `text` and `markdown` (each with a `text` string), and `actions` (an array of `id`/`label` objects). Labels and text are inert content; GUIs sanitize Markdown. No executable UI is supplied. `PresentationBlock::new` builds known views. `known()` returns a typed view or `None` for generic rendering. Unknown kinds remain opaque, preserving every byte of their JSON object. Malformed known blocks fail validation instead of falling back to opaque handling.

`native_event` and complete Presentation blocks remain raw JSON through decoding and forwarding, including numeric precision, unknown fields, and whitespace. A receiver persisting or forwarding them must retain raw slices before converting to a language's generic JSON value tree. Presentation is a view of native behavior; it never replaces the native event. Unknown native payload data is allowed; unknown Jet Event/Command kinds are rejected.

Completion is `{"kind":"completed","id":"action-1","native_conversation":"native-42"}`. The host persists native events, completions, and execution pins before acknowledging durable progress. Completion is correlated by `id` and is distinct from a transport write succeeding.

## Specification and permissions

The specification declares `schema`, stable `id`, one `harness`, a Craft `protocol` offer, `features`, `broker_permissions`, and `host_access`. Optional collections default to empty. Features have `name` and `required` (default false). The recognized features are `turns`, `actions`, and `resume`; an unknown optional feature is disabled without disabling known features, while an unknown required feature rejects the specification.

Broker permissions are closed names: `artifact_read`, `artifact_write`, and `remote_tools`. Host disclosures are tagged objects: `executable` with `name`, `filesystem` with `path`, `environment` with `name`, and `network` with `destination`. These access declarations are required in schema v1; unknown permission names or host-access kinds reject decoding. Feature flags do not grant access. The host enforces each broker operation using the originating Actor's accepted permissions and records its Security audit through the existing host pipeline. Host disclosures describe same-user access, not portable OS containment.

`requires_confirmation(accepted)` identifies added broker permissions or changed/added host targets. Installers compare against their trusted accepted specification, require renewed confirmation before expanding access, and remain subject to Security-degraded mode. The SDK only parses and reports declarations; it does not install, update, authorize, or execute broker operations.

Boundary validation follows ASVS 1.5.2, 2.2.1, 2.3.1, and 8.3.1: closed Command/action variants, bounded raw payloads, negotiation before work, and host-enforced authorization. Before converting protocol objects to dictionaries, reject duplicate known fields (especially discriminators and approval decisions); JSON Schema cannot detect duplicates once a parser discards them. Opaque native payload fields remain uninterpreted. Versions are unsigned 32-bit integers, including an explicit schema maximum. The shared corpus checks these boundaries in all three languages. TOML uses the workspace-pinned `toml` parser with its parse/serde/std features; no alternate unbounded parsing mode is enabled.
