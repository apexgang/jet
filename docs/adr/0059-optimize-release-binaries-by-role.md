# Optimize release binaries by role

Release profiles optimize every shipped executable for size: `jetd`, `jetfueld`, and bundled Crafts all build with opt-level `z` and full link-time optimization. `jetd` originally balanced speed and size with opt-level `s` and thin link-time optimization; the amendment below records why it moved. Shipped executables strip symbols into separate crash-symbol artifacts and abort on panic; library Interfaces return typed errors. Dependency default features are disabled unless explicitly justified, and both performance and size gates must pass without trading one away invisibly.

## Amendment, 2026-09-25

`jetd` moves from the balanced `release` profile to the size-first `release-small` profile (#215). Under `release` no `jetd` slice fit its 12 MiB budget (ADR-0054), even with every dependency and linker change #215 made. CI measured the stripped `jetd` with those changes, and with rustls on the ring provider #215 tried at the time, on every slice the release publishes:

| Slice | `release` (opt-level `s`, thin LTO) | `release-small` (opt-level `z`, fat LTO) |
| --- | ---: | ---: |
| Linux x86_64 | 13.32 MiB | 11,673,824 bytes (11.13 MiB) |
| Linux aarch64 | 12.39 MiB | 9,653,504 bytes (9.21 MiB) |
| macOS x86_64 | 13.36 MiB | 9,987,376 bytes (9.52 MiB) |
| macOS arm64 | 12.24 MiB | 7,640,832 bytes (7.29 MiB) |

#215 then kept rustls on its aws-lc-rs provider for the post-quantum X25519MLKEM768 key exchange. That adds 668,968 bytes to the Linux x86_64 `jetd` under `release-small`, which measures 12,342,792 bytes (11.77 MiB), 240,120 bytes under budget; the other slices had at least 2.4 MiB of room without it.

The size-first profile does not trade performance away invisibly. The workspace's development profile, which the justfile selects for the performance and resource gates, is already opt-level `z` with fat LTO. The store, startup, reconnect, and ingestion budgets of ADR-0022 and the idle-resource budgets of ADR-0055 were measured under it and passed with wide margins, as [docs/release-contract.md](../release-contract.md) and [docs/resource-budgets.md](../resource-budgets.md) record. The released `jetd` now runs the optimization settings those budgets were measured with. The `release` profile stays, because `release-small` inherits its panic, codegen-unit, and symbol settings.
