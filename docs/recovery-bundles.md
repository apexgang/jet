# Portable Recovery bundles

Issue #50 implements the core binary interface described by ADR-0074. The
Commands accept or return bounded bytes in process. A GUI transport and a CLI
file picker are separate work; the JSON Command transport does not carry these
bundles.

The exported SQLite file is one consistent `VACUUM INTO` snapshot, sanitized
and vacuumed again before encryption. It retains Conversations, Run history,
transcript Events, non-secret Settings, disabled schedule metadata, and Artifact
references. Every referenced Artifact is included by SHA-256. Craft identities,
versions, and supported Harnesses are metadata only; import installs no code.

Account bindings, Credential references, native-resume state, Plane and Client
identities, Pairings, audit chains, command receipts, execution plans, queued
work, service ownership, Project repositories, and Workspace directories are
excluded. Workspace patches are not selected by this interface. Credentials
embedded by a user in their own transcript or Artifact content remain content;
Jet's Credential stores and native authentication files are never read.

The age v1 stream authenticates the manifest and every component. Component
lengths and SHA-256 hashes are also checked, including for explicitly unencrypted
bundles. Unknown format versions, unexpected component names, duplicates,
trailing bytes, missing Artifacts, and incompatible SQLite schemas are refused.
Only this build's SQLite schema and Event payload version 1 are supported.
The complete bundle is limited to 256 MiB and 4,096 components. Larger exports
fail rather than silently omitting history or payloads.

Import consumes the entire authenticated stream before writing private scratch
files. It assigns new Conversation identities and retains schedules as disabled
metadata. After all checks pass, it publishes an owner-only `recovered-*`
directory under `recovery/imports`, together with provenance and a durable
Command receipt in `recovered.json`. Retrying the same import Command returns
the original result, including after restart; reusing its identity for different
bundle bytes is refused. Decryption keys and request encoding are excluded
from the receipt digest to avoid retaining a fast password verifier. The receipt lives as long as that archive. Disk admission
reserves the payload, SQLite rewrite scratch, and metadata before staging.
The live Plane database, Deletion ledger, and Authority fences are not replaced.
`Store::open` refuses the Recovered SQLite marker, so a staged copy cannot become
a Home Plane by accidentally selecting its file. These archives have no live
execution or automatic retention worker.

Losing the passphrase or recipient private key makes an encrypted bundle
unrecoverable. SQLite scratch files and staged Recovered copies use owner-only
filesystem access and the host's disk encryption, as specified by ADR-0075.

Encryption is selected with `RecoveryProtection::passphrase` or
`RecoveryProtection::recipients`. The latter accepts only native X25519 public
recipients. Passphrases use scrypt with N = 2^18; import refuses a higher work
factor rather than allocating unbounded KDF memory. Secrets have redacted Debug
output and Command encoding. Neither their values nor password hashes are
persisted.

An unencrypted export requires the exact `RECOVERY_PLAINTEXT_WARNING` disclosure
passed to `RecoveryProtection::unencrypted`. The Command carries that
acknowledgement, and `recovery.unencrypted_export_authorized` reaches the
Security audit before plaintext is returned. Security-degraded mode refuses
this exception and imports, while encrypted export remains available. Plaintext
import requires `RecoveryKey::unencrypted`; a wrong decryption key never selects
plaintext mode.

Run `just recovery-crypto-budget` from `packages/` to enforce the age adapter's
incremental two-MiB executable budget. It compares two stripped executables
built with the repository's development profile, one with both permitted age
modes and one with the shared error type only. Release packaging must still
check the complete executables against ADR-0054. Authenticated Recovery adds
cryptography intentionally; the budget confines that addition without linking
SSH recipients, plugins, or ASCII armor.

Implementation was divided into two reviewable commits: the Store snapshot
sanitizer, validation, and Recovered marker first; then the encrypted core
Commands, audit integration, tests, and binary budget. The latter depends on
all three Store operations to export complete data and publish inert imports.
