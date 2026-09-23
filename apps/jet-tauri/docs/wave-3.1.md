# Wave 3.1: Planes and pairing (Tauri / Linux)

This document is being written slice by slice. Slice 5 completes the Delivered,
Security model, Remaining backend dependencies and Verification sections. The
new-crates record below was written with slice 4, the slice that added them.

## New crates

Slice 4 added this computer's Ed25519 client identity for remote Planes, and
stored it in the OS secret store (ADR-0076).

| Crate | Version in lock | Why |
|---|---|---|
| `ed25519-dalek` (`fast`, `zeroize`, no defaults beyond those) | 3.0.0 | Makes the key from a seed, signs remote logins and pairing transcripts. Same version as `packages/Cargo.lock`. |
| `secret-service` (Linux only, `rt-async-io-crypto-rust`) | 5.2.0 | Freedesktop Secret Service over D-Bus with pure-Rust session crypto. `openssl` stays off. |
| `zeroize` | 1.9.0 | Wipes seeds, the retained claim code and the buffers Secret Service returns. |
| `sha2` | 0.11.0 (moved from 0.10) | Authentication strings and key fingerprints. Only raw digest bytes are used; 0.11 dropped hex `Display`. |
| `getrandom` | 0.3.4 (already in the lock) | Seed and probe-secret generation. |
| `tokio` feature `process` | | The shell spawns the system `ssh` itself, so it can classify the exit status. |

`secret-service` also brings in `aes 0.9`, `cbc 0.2`, `hkdf 0.13`, `hmac 0.13`,
`hybrid-array 0.4`, `num 0.4` and `getrandom 0.4` (0.4 was already in the lock).
`ed25519-dalek` brings in `curve25519-dalek 5`, `ed25519 3`, `signature 3` and
`subtle`.

### `cargo tree -d`

New duplicate versions compared with the tree before slice 4:

- `sha2` 0.10.9 and 0.11.0, and with them `digest` 0.10/0.11,
  `block-buffer` 0.10/0.12, `crypto-common` 0.1/0.2 and `cpufeatures` 0.2/0.3.
  Nothing in this app can remove the 0.10 line: `jet-protocol` (`packages/`)
  and `tauri-codegen` depend on `sha2 0.10`, and `ed25519-dalek 3` needs
  `sha2 0.11`.

No other new duplicates.

### zbus features (`cargo tree -e features -i zbus`)

zbus stays 5.19.0 and keeps the same runtime: `async-io` (from `notify-rust`)
and no `tokio`. The one new zbus feature is `blocking-api`. `secret-service`
always enables it (`default-features = false, features = ["blocking-api"]` in
its manifest), and it only makes `zbus_macros` generate blocking proxy types.
It adds no executor and does not change how zbus picks its runtime. Both
`secret-service` runtime features map to `zbus/async-io` alone.

Before:

```text
zbus feature "async-executor", "async-fs", "async-io", "async-lock",
"async-process", "async-task", "blocking"   (all from notify-rust "async")
```

After: the same list, plus `blocking-api` (from `secret-service`). The
`rt-async-io` and `rt-async-io-crypto-rust` features of `secret-service` also
enable `zbus/async-io`, which was already on.

### Size

Release binary `jet-tauri` (`cargo build --release --locked`, Linux x86-64,
unstripped, both built with the same frontend bundle so only Rust code
differs):

| | Bytes |
|---|---|
| Before slice 4 | 25,442,016 |
| After slice 4 | 28,477,304 |
| Difference | +3,035,288 (about 2.9 MiB, +11.9%) |

No size budget is defined for the Tauri app, so there is nothing to pass or
fail against. The increase includes all of slice 4's code, not only the
crates: the SSH transport, enrollment, the registry file and the secret
store.

### Desktop notifications re-test

Desktop notifications use zbus through `notify-rust` too, so the manual
re-test in the §8.3 matrix, step 10, is still needed. It needs a desktop
session with a connected remote Plane and is recorded with the slice 5
verification. The automated gates cannot show it.
