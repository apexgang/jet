# Credential store

Issue #68 implements the observation behind ADR-0076: a Plane tells a
locked credential store from an open one and from none at all by speaking
to the store itself, and it can prove the store holds a Credential before
durable Pairing depends on it (ADR-0106).

## What the Plane looks at

| Platform | Backend | Observation | Verification |
| --- | --- | --- | --- |
| Linux | freedesktop Secret Service, over the session bus | The default collection's `Locked` property | `CreateItem`, `GetSecret`, `Delete` in a plain session |
| macOS | Keychain, through the Security framework | Unlocking the default keychain without a password, with dialogs disabled | A generic password added, read, and deleted |

The session bus is the one the environment advertises, or the socket the
session manager leaves in the runtime directory. A `jetd` started without
a session has neither and reports the store unavailable.

## What never happens

`jetd` has no person to ask for a secret and no place to keep the answer,
so the probe never prompts and never starts a store (ADR-0076):

- Every Secret Service call carries `NoAutoStart`. A store that is
  installed but not running is reported as unavailable rather than
  activated, because activation can open a first-run or unlock dialog.
- No Prompt is ever answered. A write that needs one is reported as
  locked.
- Keychain calls run with user interaction disabled. A call that would
  have opened the unlock dialog fails with `errSecInteractionNotAllowed`
  instead, and that failure is what "locked" means on macOS. The setting
  is process-wide, so Keychain calls are serialized.
- A store that does not answer within five seconds is unavailable. A
  probe that waited on it would stall every Command that revalidates the
  Capability.

## Observation and verification are separate

The Capability snapshot's `credential_store` is a read: it happens at
startup, whenever a client asks for a fresh snapshot, and before every
Command that depends on the store. None of those should write into a
person's keyring, so the snapshot never does.

The `verify_credential_store` Query (Jet 1.44) is the round trip. The
Plane creates one probe item filed under `me.heeka.jet.credential-store`
with the account `probe`, reads it back, and deletes it, and reports how
far it got:

| Outcome | Meaning |
| --- | --- |
| `verified` | The store held the item, returned it unchanged, and let it go. |
| `locked` | The store would need the user to unlock it. Nothing was created. |
| `unavailable` | The store, or a default collection to keep an item in, cannot be reached. |
| `failed` | The store answered but did not complete the named step: `create`, `read`, or `delete`. |

The probe value is random and disposable, so it travels in a plain
Secret Service session. The item is deleted whatever the read said, and a
Prompt the store offers is dismissed rather than answered; only a store
that will not delete without one keeps the item, and that is what
`failed` at `delete` reports. Verification leaves the last observed
snapshot as it was; a client that wants the store's state after unlocking
it asks for a fresh observation.

On macOS the lock state is read by asking the default keychain to unlock
without a password: a no-op when it is open, a refused dialog when it is
locked. `SecKeychainGetStatus` would read the state directly, but the
Security framework crate does not bind it and the workspace forbids
unsafe code.

The GUI's Set up secure storage flow asks for this verification after a
provider is installed, and durable Pairing (#13) gates on a `verified`
answer. `jetd` only reports and rechecks; it never installs a provider.

## Tests

Nothing in the test suite touches the developer's keyring. `jet-core`
serves a Secret Service of its own over a private socket, with the lock
state, read fidelity, and delete behaviour the test chooses, and the
production client speaks to it exactly as it speaks to a session bus. The
`jetd` conformance tests do the same for a Plane under test. macOS
behaviour is covered by compilation against the Security framework; the
Keychain of the machine running the tests is what a macOS `jetd` under
test observes.
