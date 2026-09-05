# Remote Jet connections

`Client::connect_ssh` launches the system `ssh` client with a validated
`[user@]host` destination and the fixed remote command `jetd connect --stdio`.
The target must already have a running `jetd serve`. Normal login requires
an enabled Pairing for the installation. `ClientIdentity` supplies signatures from the
installation's platform credential store without exporting its private key.
`Client::connect_remote` supports an already authenticated SSH byte stream.

SSH configuration still supplies the endpoint, user, port, identity, agent,
jump hosts, and known-hosts files. Jet forces strict host-key checking,
disables DNS host-key acceptance and connection-master reuse, and never
confirms an unknown or changed key. Resolve host trust explicitly outside
Jet before connecting. These options follow the system
[SSH client](https://man.openbsd.org/ssh.1) and
[SSH configuration](https://man.openbsd.org/ssh_config.5) contracts.

The stdio helper owns no Plane state. It prepends an internal remote marker
before forwarding bytes to the owner-only daemon socket. It cannot forward
remote bytes as a locally authorized connection. Protocol stdout contains
only frames; startup failures go to stderr. Closing either direction ends
the relay, including when the peer keeps stdin open.

Protocol 1.7 requires the normal preface and ClientHello, followed by a
server `challenge` containing a fresh 32-byte nonce. Normal login answers
with a ConnectionProof; enrollment uses the restricted path below. Ed25519 signs, in order:

1. `jet.connection.v1`, a zero byte, `ed25519`, and a zero byte.
2. The canonical ClientHello encoded by `encode_control`.
3. The raw nonce bytes.

New installations use `Client::pair_remote` and a RemotePairingRequest in
place of a ConnectionProof. Only claim and completion are admitted. A
successful claim returns the confirmation string and bound Pairing signing
material, so enrollment needs no protected status Query. The target owner
confirms the displayed string through an authorized connection. Completion
occurs on another short restricted connection; the new client then logs in
with a fresh connection signature. Each enrollment reply closes its
connection without granting application streams.

The nonce is single-use within a ten-second server handshake deadline.
Welcome and application streams follow only after strict verification
against the stored enabled key. Every reconnect repeats authentication;
there is no bearer token. The existing local protocol remains compatible
with older minors. Authentication outcomes enter the Security audit without
the nonce, signature, or key material.

Disable, revoke, and replacement Pairing invalidate every connection for
that Client identity after the Command commits. Invalidation is serialized
with admission; replaying an old Command receipt has no new side effects.
Queued requests must still hold live authority, and idle or blocked
connection loops are canceled. Existing connection authority cannot revive
when a key is enabled again.

`Core::spawn_no_visa` admits an operation under this live authority. It
creates a separate process group, requests SIGTERM on revocation, and forces
SIGKILL after two seconds, with at most one more second awaiting reaping.
Descendants receive the full grace period even if the leader exits early.
Visa Runs do not use this
connection-owned scope. Issue #32 will connect the file, Git, terminal, and
process remote tools to this seam after validating registered roots and
permissions; this change does not expose an unrestricted execution command.

Securability notes: authenticity, accountability, and bounded process
lifetime shape these interfaces. Relevant controls are ASVS 1.2.5, 6.3.4,
8.3.2–8.3.3, 11.5.1, 11.6.1, and 16.2.1–16.2.5. The daemon limits concurrent
connections to 128. SSH and Jet authentication remain independent checks.
