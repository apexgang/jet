# Managed Run execution

Issue [#20](https://github.com/apexgang/jet/issues/20) adds `start_run` and `run_execution` in Jet protocol 1.12. A start request names a Conversation, an installed Craft identity, and initial input. Core validates its registered Project and canonical Workspace or Local checkout, reobserves Git capability, verifies the accepted Craft digest and declarations, and commits the Run, immutable launch plan, Events, Command receipt, and start Effect together. The response contains the Run in `starting`; external work follows the commit.

```json
{"type":"start_run","conversation_id":"<uuid>","craft":"fake","prompt":"Make a change"}
```

```json
{"type":"run_execution","run_id":"<uuid>"}
```

The execution snapshot returns a fenced Run, optional active activity, Managed processes, native Conversation identity, and native exit code. Run lifecycle is authoritative independently from activity. The Craft may report working, waiting for user, approval, authentication, quota, or reconnection only while active. A native turn completion retains its Conversation identity; only a native process end terminates the Run. Client-driven lifecycle transitions are rejected for managed executions. Legacy `create_run` and `transition_run` retain their existing non-executing record behavior.

The Event journal includes lifecycle, activity, and process changes. Harness output, activity, and native identity carry a Harness Actor tied to the pinned Run; process supervision carries a Run-supervisor Actor. Admission retains the initiating client separately. For rollback compatibility, journal actor columns and the wire `actor` field retain that client authorization; additive `_jet_origin` journal metadata and the wire `origin` field carry the actual responsible origin. Current consumers must prefer `origin` when present. The Core plan stores the Adapter contract opaquely, and `jetd` alone translates Craft/helper protocol DTOs. `run.output` carries `native_json` and `presentation_json` strings containing the original JSON bytes. This preserves large integer precision, whitespace, and opaque Presentation blocks through the existing journal's JSON value representation. A semantic output observation accepts at most 128 KiB of combined JSON and 128 Presentation blocks; larger content must use artifact references. An oversized observation remains unacknowledged at the helper. Event query pages also enforce a byte budget. Consumers decode those strings as inert content, with the same generic rendering and Markdown sanitization rules as Craft Presentation blocks.

## Accepted Crafts

Until the installation workflow is implemented, the Plane owner provisions `~/.jet/crafts/<id>.json`. The identity contains only ASCII letters, digits, hyphens, or underscores. This is trusted installation metadata, not a client-supplied executable or a grant derived from a peer handshake.

```json
{
  "executable": "/absolute/path/to/craft",
  "sha256": "<SHA-256 of the accepted Craft executable>",
  "specification": {
    "schema": {"major": 1, "minor": 0},
    "id": "fake",
    "harness": "fake",
    "protocol": {
      "family": "craft",
      "versions": [{"major": 1, "minor": 1}],
      "capabilities": ["runs"]
    },
    "features": [{"name": "turns"}],
    "broker_permissions": [],
    "host_access": [{"kind": "executable", "name": "/absolute/path/to/harness"}]
  }
}
```

The `jetd` transport Adapter starts the executable with `--socket <private endpoint>`. It serves execution connections with the Craft SDK; one process multiplexes Runs using the same accepted artifact digest. Each handshake must exactly match the accepted specification and negotiated protocol. The plan persists Craft 1.1 and helper 1.0 pins before dispatch.

Craft 1.1 adds the `runs` capability, `start` and `acknowledge` Commands, and `run_started`, `run_launch_failed`, `activity`, `progress`, and `run_ended` Events. Existing Craft 1.0 turns/actions retain their contract. The initial input identity is the Run UUID. The host provisions one Run-role helper and supplies its socket through `start`; the Craft launches the native Harness over that helper connection and translates native source into semantic Events. It sends `progress` only after every semantic Event corresponding to the source record. The host acknowledges only after those Events and the source cursor are committed. The Craft then acknowledges that offset to the helper.

`jetfueld run --config <owner-only config>` owns the native process, its standard input/output/error, and its terminal OS result. Helper messages are independently negotiated framed JSON on a private Unix socket; their Rust DTOs in `jet-protocol/src/helper.rs` define the language-neutral contract. Launch arguments are an array, initial input uses stdin, and the working root and accepted executable disclosures come from the host-written configuration. Native output remains opaque to the helper. Its disk spool retains at most 64 MiB of unacknowledged records and applies backpressure at the bound. A definite launch rejection produces a terminal `launch_failed` record and fails the Run without retaining admission exclusion. The helper exits after its terminal record is acknowledged; disconnecting its Craft leaves native work and source records intact.

## Validation and scope

`just test` builds the separately deployed helper before nextest. `jet-daemon/tests/runs.rs` exercises real daemon → Craft → helper → Harness processes in a temporary Git Workspace. It observes each active attention reason and distinct process identities, verifies failed Craft/Harness launches and nonzero native exits, checks prior-minor Event decoding, then verifies completion, exact Command replay, Event history, and the Workspace file after daemon restart. Core boundary tests cover Project admission, durable retries, rejection of client lifecycle changes, and failed capability revalidation before execution.

Issue [#21](https://github.com/apexgang/jet/issues/21) owns restart adoption, authoritative descriptor validation, orphan handling, reboot loss, Craft recovery, and idle Craft shutdown. This implementation does not automatically adopt an interrupted execution or retry an ambiguous launch. A disconnected active Run reports reconnection; its helper retains source for that recovery work. Turn queue and execution controls remain in #22 and #23.

The implementation crosses the protocol, store, core, daemon, and helper deployment boundaries. The smallest independently reviewable stages are the additive Craft/helper contracts, the helper executable, durable admission/projections, and the integrated supervisor/conformance test. Generated GUI contracts and SQLx cache changes are mechanical. The complete subprocess-and-restart acceptance path requires these stages together; each new implementation module remains below the repository's 500-line target.

Security constraints that shaped this work are **Integrity**, **Resilience**, and **Observability**: host-owned launch roots and accepted declarations (ASVS 2.2.3, 5.3.2, 8.3.1), argument-array process creation (1.2.5), and transactional lifecycle/Event updates (2.3.3). No new third-party dependency was introduced; crates use the workspace's existing dependencies. Crafts are trusted same-user code, as defined in ADR-0098; executable disclosures are enforced by the helper and are not represented as OS sandbox containment.
