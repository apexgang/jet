# Craft lifecycle

`install_craft` publishes a verified Artifact as the default for subsequent
Runs. It requires the complete discovery confirmation on updates, including
broker permissions and declared host access. Concurrent or unresolved
publications must settle before another update is accepted. Each published
digest remains content-addressed, and installation plans retain old Artifacts.

An active Run keeps its accepted digest and negotiated Craft protocol until
termination. A subsequent Run selects the current compatible default. If that
default is incompatible, queued input waits. A running Craft multiplexes the
Runs pinned to its digest; old and new digests can run concurrently.

Jet protocol 1.27 adds `disable_craft`, with `craft_id` and a closed `mode`:

- `wait` blocks new Runs, including queued continuations, while active Runs
  finish using their accepted Craft.
- `force` also terminates the Craft and marks its Runs `needs_attention`.
  Their Harnesses and replay spools remain under `jetfueld`. Interactive
  execution inspection and termination remain available.

Both modes persist across daemon restarts and require an authenticated
interactive client. Reinstalling does not clear a disable. A later `wait`
request does not downgrade an existing `force` barrier.

Crashes reconnect at the same digest and source offsets. The process manager
applies per-digest delays of two, four, and at most eight seconds; Core also
bounds unsuccessful recovery attempts before requiring intervention. A Craft
with no active Runs receives SIGTERM after five idle minutes, with forced
termination if it does not exit within one second. Helpers are independent.
A private supervisor owns each Craft and reaps it when the daemon's control
pipe closes, including after a daemon crash. Stable per-digest locks serialize
replacement processes across daemon restarts. Force disable also stops idle
versions of the disabled Craft.

## Signed revocations

Deployments supply the trusted Jet release Ed25519 public key with
`jetd serve --release-verification-key /absolute/path/to/release-key`.
The file contains exactly 32 raw bytes. Provision it separately from Crafts
and downloaded metadata. This repository does not embed a production release
key or fetch a third-party denylist.

Jet reads `crafts/revocations.json` under its home at startup and during
lifecycle reconciliation. Release tooling must publish the file atomically.
Its JSON object contains `digests`, a sorted, unique list of lowercase SHA-256
hexadecimal strings, and `signature`, a JSON array of 64 byte values. The
signed bytes are the ASCII prefix `jet.craft-revocations.v1`, one NUL byte,
then each digest followed by one newline. Files are limited to 512 KiB and
4,096 digests.

Only a valid signature under the configured release key can add revocations.
Invalid metadata is ignored with a diagnostic. Accepted digest revocations
are permanent store records, so removing metadata, replaying an older list,
or restarting without the key cannot undo them. Revocation force-disables
the affected digest automatically and prevents launch or recovery with it.
Each newly accepted digest commits one Security-audit record attributed to
Jet's internal revocation authority. Repeated metadata does not duplicate it.
Clients older than protocol 1.27 receive an upgrade error when an audit page
contains this new actor, rather than misattributing it to an interactive user.
Other digests of the same Craft remain eligible unless the user also disabled
the Craft identity.
