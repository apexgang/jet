# Swift transport

Wave 0.2 establishes the native client's non-UI boundary to `jetd`. The
implementation lives under `apps/jet/jet/Client/`; feature code uses
`JetClient` and does not see Network.framework callbacks, frame headers, JSON
dictionaries, or native error text.

The cross-client architecture and frozen Wave 0 boundary are recorded in
[ADR-0106](adr/0106-freeze-independent-desktop-client-foundations.md).

## Adapter decision

The macOS 26 SDK's structured-concurrency Network API was evaluated with
`NWEndpoint.unix(path:)`. Its closure-scoped connection lifetime is useful for
one bounded exchange, but Jet needs one connection to outlive many concurrent
Queries, durable Commands, Event subscriptions, cancellations, and reconnects.
Keeping that lifetime alive would still require an owning task plus a mailbox.

Wave 0.2 therefore uses `NWConnection` behind one `JetUnixSocketTransport`
actor. The actor is the only callback bridge. It exposes exact-length async
reads, ordered writes, and close. A separate byte-transport protocol exists
because the hermetic `jetd` test is a real second adapter, not as a speculative
abstraction.

The sandboxed macOS target has an exact home-relative exception for
`~/.jet/runtime/jetd.sock` and outgoing client authority. The exception does not
grant the app access to the Plane store, diagnostics, Workspaces, or the rest of
`~/.jet`. Distribution metadata must disclose the temporary exception if the
Mac app is submitted through App Store Connect.

## Protocol boundary

`JetClient` owns the connection state machine, handshake, negotiated frame
limits, numbered streams, the bounded pending-reply registry, and reconnect
policy. It currently exposes the Wave 0.2 proof surface:

- Plane status Query;
- clear-setting Command with a caller-visible stable command ID;
- ordered Event pages and an `AsyncThrowingStream` that resumes after the last
  yielded cursor;
- explicit disconnect and stable connection state.

Queries retry after transport loss. A durable Command retries only with the
same command ID and byte-equivalent command object. Cancelling a Query returns a
stable cancellation error. Cancelling a Command reports an unknown outcome; it
does not claim to stop or roll back daemon work (ASVS 2.3.1, 2.3.3).

## Validation and errors

The generated `jet-v1.schema.json` is copied into the app bundle by
`just contracts`. Every incoming handshake and server message is bounded,
parsed, and validated against that schema before fields become Swift values
(ASVS 1.5.2, 2.1.1, 2.2.1, 2.2.2). The parser enforces the protocol's depth and
collection limits, rejects duplicate known wire fields, and leaves opaque raw
payloads uninterpreted. Event actor, origin, and payload values retain their
exact UTF-8 fragments.

Server error category, code, retryability, and recovery actions become
`JetPresentationError`. Native Network, JSON, and operating-system error text
never crosses the adapter boundary (ASVS 13.4.1, 16.5.1).

## Verification

`JetTransportIntegrationTests` proves:

- a real Network.framework UNIX-domain socket round trip;
- local handshake, framed Query and Command routing, and numbered streams;
- a lost Command reply reconnects with byte-equivalent command content;
- ordered Event delivery survives another disconnect and resumes from cursor;
- request and stream cancellation complete without claiming daemon rollback;
- exact raw JSON retention and frame-allocation limits;
- every shared `jet-fixtures.json` accept/reject decision in the production
  validator, including concurrent use of the immutable schema graph.

Remote SSH transport, Keychain-backed signing, and feature-specific typed
Queries and Commands remain later slices. They reuse this client boundary and
must not bypass schema validation or stable error mapping.
