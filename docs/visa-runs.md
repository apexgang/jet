# Visa Runs

Issue [#31](https://github.com/apexgang/jet/issues/31) adds `start_visa_run`
in Jet protocol 1.25. Connect to the selected destination using the ordinary
local or paired remote Jet connection. That Plane must already own the
Conversation: ADR-0062 makes the Visa destination its Home Plane. Changing
it requires an explicit Plane transfer.

```json
{"type":"start_visa_run","conversation_id":"<uuid>","destination_plane_id":"<uuid>","account_binding_id":"<uuid>","craft":"codex","prompt":"Make a change"}
```

The destination validates its Plane identity, registered Project and working
tree, Account binding, accepted Craft artifact and protocol, and required
capabilities. Admission and dispatch revalidate the selections. The accepted
selection stays pinned for queued continuation; losing its binding never
selects another account or uses the GUI's credentials.
Visa launch plans use format version 2 so an older daemon refuses new
execution instead of ignoring the binding selection. Legacy managed Run plans
retain version 1.

Visa execution uses the destination Harness's native authentication. Its
Account binding must use `harness_native` and match the accepted Harness's
Provider. The bundled Codex and Claude Code mappings are supported. Other
credential sources and unknown Provider mappings return explicit capability
refusals: managed Runs cannot yet deliver platform-store, external-helper,
or session-only credentials. Account-binding snapshots retain their existing
credential-store waiting, unavailable-storage, and session-invalidation states.
Authentication requested by a running native Harness is reported as
`waiting_for_auth`.

The Harness, Craft, `jetfueld`, filesystem, extensions, native checkpoints,
and sandbox all remain on the Home Plane, using the existing managed Run
pipeline. Jet does not change native extension formats or claim additional
sandbox containment. Run lifecycle, activity, processes, output, and Change
checkpoints use the ordinary protocol snapshots and Events.
`run_execution.visa` records the selected Plane and binding identities, including
after restart or completion. Older negotiated minors omit that additive field
and refuse the new launch Command.

A disconnected GUI loses observation, not ownership. It reconnects to the
same Plane and retries an uncertain Command with the same identity and
selections. Daemon restart reconnects the retained helper; proven execution
death becomes `lost`; uncertain launch outcomes are never automatically
reissued. Revoking a client stops new admission but does not terminate its
already admitted Visa Runs.

Security boundaries emphasize Integrity, Authenticity, and Resilience:
destination-owned identifiers and live binding checks apply ASVS 2.2.3 and
8.3.1–8.3.2; explicit credential refusals apply ASVS 16.5.3. The request and
durable selection contain identities only. No new dependency was added.

The change can be reviewed in three connected stages: the additive wire
contract and generated client models; destination admission and immutable
selection revalidation; and protocol conformance in `jet-daemon/tests/visa.rs`.
The conformance test uses separate real Plane stores, authenticated stdio
connections, and controlled native Craft/helper/Harness processes. Those stages
together establish the remote execution acceptance path.
