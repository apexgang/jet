# Third-party Craft installation

Jet protocol minor 23 exposes third-party Craft discovery and installation as
two separate operations. Discovery is read-only. It resolves a source, parses
its bounded specification, selects the Artifact for the Plane, and returns an
exact confirmation object. Installation accepts only that complete object and
repeats discovery before committing anything.

## Verified GitHub releases

A public release source names an exact `owner/jet-craft-<id>` repository and
tag. Jet resolves the tag to a 40-character Git commit, reads
`.jet/craft-spec.toml` at that commit, and accepts only a prebuilt release
asset whose GitHub-provided SHA-256 matches the specification. Repository,
tag, commit, Craft identity, version, platform, Artifact name, size, and digest
must all agree. Metadata is capped at 1 MiB, the specification at 64 KiB, and
an executable Artifact at 64 MiB. Fetches require TLS, follow at most three
redirects, and remain on GitHub-owned hosts.

The published specification extends the runtime Craft specification with:

```toml
version = "1.2.3"
publisher = "Example Org" # a publisher claim, not a Jet endorsement

[[artifacts]]
name = "jet-craft-example-macos-aarch64"
operating_system = "macos"
architecture = "aarch64"
sha256 = "<64 lowercase hexadecimal characters>"
```

Unknown required features reject discovery. Unknown optional features are
disabled and omitted from `enabled_features`. Broker permissions and host
access are closed declarations: an unknown value is rejected rather than
treated as optional authority.

## Explicit consent

The discovery response contains the stable Craft identity and version, the
enabled known features, and a confirmation that includes:

- the exact source;
- canonical repository identity;
- the publisher's unendorsed claim;
- pinned commit and Artifact SHA-256;
- every requested broker permission;
- every disclosed same-user host access; and
- whether the executable is a verified release or an unverified developer
  source.

Clients must show that surface to the interactive user and return it
unchanged. Jet rejects edited confirmations, moving releases, changed files,
and changed downloads. Acceptance and refusal are recorded against the Craft
in the owner-only Security audit. Before the transaction, Jet streams the
Artifact into an owner-only temporary, verifies its size and SHA-256, syncs
it, and atomically publishes it by digest under `~/.jet/crafts/artifacts`.
The accepted transaction records a durable idempotent Effect that creates the
Craft manifest without overwriting an existing installation.

Hash verification establishes Artifact identity; it does not sandbox the
process. A third-party Craft remains an arbitrary same-user executable and may
have the operating-system access disclosed in the confirmation (ADR-0098).

## Developer Mode

Local and source-built Artifacts are refused while the Plane-wide
`craft.developer_mode` Setting is false, which is its built-in default.
Enabling or clearing that Setting is itself recorded as a Security decision.
When enabled, Jet reads the explicitly named specification and Artifact,
checks the same schema, feature, platform, and SHA-256 declarations, and labels
the confirmation `developer_source`. Developer Mode removes release
provenance; it does not bypass the exact-confirmation, digest, permission,
host-access, or same-user trust disclosures. Jet rechecks Developer Mode in
the transaction that accepts the installation.
