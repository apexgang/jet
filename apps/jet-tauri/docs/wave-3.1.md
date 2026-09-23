# Wave 3.1: Planes and pairing (Tauri / Linux)

This app now works with more than one Plane. It lists this computer and up to
16 remote Planes reached over SSH, pairs itself with a remote Plane, and lets
the owner of any connected Plane pair, disable and revoke other computers.
Recent and Search combine every Plane and name the Plane on each row. A Plane
that fails or runs an older Jet is shown as such, and the other Planes keep
working.

## Delivered

**Plane registry.** The shell holds the local Plane ("This computer") and up to
16 remote Planes. A remote Plane is written to `planes.json` in the app data
directory only after a login proved its identity. The file holds the native
handle, the SSH address, the Plane identity, the credential kind (durable or
this session only) and the time it was added. It holds no key material, codes,
cursors or ssh options. It is written atomically with mode 0600, capped at
16 KiB, and read with unknown fields refused. An unreadable file is kept as
`planes.json.invalid`, the registry starts empty, and the Planes panel says so
once.

**Explicit Plane arguments.** Every conversation-, run-, work-panel- and
delivery-scoped command takes a `planeId`: `local` or a UUID the shell issued.
The webview never sees a socket path, SSH argument or key. Reviews, grants,
file bindings, terminals and client-change reviews store the Plane binding
(handle and Plane identity) they were prepared against. They run only against
that binding; after Forget, or when the address now reaches a different Plane,
they are refused with `plane.review_moved`. Project setup and new tasks stay on
this computer in 3.1.

**Owner pairing.** On any connected Plane, the Planes panel opens and closes
the pairing gate, opens a one-time manual-code offer, and confirms a claim by
having the owner type the 6-digit string shown on the other computer. jetd
compares it; the owner view never receives the string, so confirmation cannot
be clicked through. Paired computers are listed with a key fingerprint, the
client ID prefix and the paired date, and can be disabled, enabled or revoked
after a review dialog that names the Plane label and identity. The same
controls work on a remote Plane over its SSH connection, which is how a
headless Plane pairs more computers after its first pairing. Controls are
paused, with the reason shown, while a Plane is Security-degraded or its store
is read-only; Stop pairing, Reject and Close pairing stay available because
they only close the gate.

**Enrollment.** Add a Plane takes an SSH address, logs in if this computer
already has a key the Plane accepts, and otherwise walks through the code, the
authentication string and Finish. Pair again repairs an existing entry in place
and keeps its handle, so selections and bindings survive. Revoke-then-forget
revokes this computer on the Plane before removing it locally.

**Per-Plane feeds and aggregation.** Each Plane has one feed with its own
cursor, its own notification fence and its own failure state. Recent walks each
Plane's page chain, keeps the newest 4,096 rows per Plane and merges them
newest first by creation time, then Plane identity, then Conversation ID. Search
queries every Plane and shows one group per Plane in that Plane's rank order,
without interleaving. A failed, offline or unsupported Plane gets its own row
or group. The Planes badge counts Planes that need attention; the status block
shows "This computer · Connected" or "2 of 3 Planes connected". The Plane
detail has a feature table, the only place protocol minor numbers appear.
Elsewhere an unsupported feature reads "Update Jet on {label}".

## Security model

- **SSH standard I/O only.** A remote Plane is reached only by spawning the
  system `ssh` with the fixed arguments from `SshEndpoint::command()`:
  `StrictHostKeyChecking=yes`, `BatchMode=yes`, no forwarding, no master
  reuse, `--` before the address, and the fixed remote command
  `jetd connect --stdio`. The webview supplies only the address, which is
  validated as data. ssh's stderr goes to `/dev/null`. There is no TCP,
  socket-forwarding or plaintext path to fall back to: a remote transport can
  only be built from an `SshEndpoint`. Jet never accepts a new or changed host
  key; the user connects once with `ssh <address>` in a terminal first.
- **Single-flight connects.** Loads, searches and feeds for one Plane share one
  connection attempt, so they never start parallel ssh processes. A refusal
  that retrying cannot fix (for example `connection.unauthorized`) is sticky:
  no ssh is started again until the user chooses Retry or Pair again. Other
  failures back off at 1, 5, 15 and 30 seconds. This bounds the failed
  logins recorded in the target Plane's Security audit.
- **Key storage (ADR-0076).** This computer's Ed25519 seed lives only in the
  Secret Service, and only after a create, read and delete probe succeeded.
  The seed is created at the first claim, so local-only use never touches the
  keyring. Without a working Secret Service the user can choose "Pair for this
  session only": the seed stays in process memory and is wiped when Jet quits,
  and the Plane then shows "Pair again". There is no file fallback, and the
  webview never receives the seed, a proof or a signature.
- **Unlock before ssh.** The key is loaded, with the keyring's unlock prompt if
  needed (up to 60 seconds), before ssh starts, because the server nonce is
  valid for 10 seconds. The loaded key serves one handshake or one pairing
  signature and is wiped when dropped. A dismissed or timed-out prompt gives
  `identity.secret_store_locked` and starts no ssh.
- **Pairing transcript.** The claim response is validated before anything is
  signed, and the authentication string is computed on this computer from the
  validated transcript. It is never the Plane's copy.
- **The one-time code.** On the owner side the code crosses into the webview
  only to be shown, is disclosed once (a retried open answers "already
  shown"), and is cleared when the offer ends, the Plane changes or the panel
  closes. On the enrolling side the typed code is capped at 32 characters,
  zeroized after use, and at most 4 drafts, 4 claims and 4 tickets are held.
- **Stop pairing always closes the gate**, including a gate another client
  opened. Closing the gate is the only way to end an offer today.
- **Command IDs.** A Command ID is kept per exact body, and the key includes the
  Plane. An uncertain outcome (transport loss, timeout) retries with the same
  ID and body. A definite refusal is stored as the receipt and the next
  attempt gets a new ID (ADR-0093). Maps are capped (64 pending commands per
  kind, 32 client-change reviews).
- **Changing this computer's own access.** Disabling or revoking this computer
  over the same remote connection ends that connection, which races the reply.
  The shell reconnects once with the same Command ID. If the Plane then
  refuses the login, the receipt is `applied_unverified`: the change almost
  certainly happened but cannot be read back from here. The Plane is marked as
  no longer allowing this computer and the dialog offers Forget.
- **What Forget does.** Forget removes the Plane from `planes.json` and drops
  its feed, notification fence, pairing command IDs, drafts and tickets on this
  computer. It sends nothing to the Plane: this computer stays paired there
  until an owner revokes it, which is why revoke-then-forget exists. Prepared
  reviews and bindings for that Plane are kept and answer `plane.review_moved`.
  The key in the Secret Service is kept, because it is this computer's single
  identity for every remote Plane.

## Remaining backend dependencies

- `pairing_owner_cli`: jetd has no pairing subcommand, so the **first** pairing
  with a Plane needs a Jet client running on that Plane's own computer. After
  that, any paired client can open offers and confirm claims remotely.
- `pairing_transcript_helpers`: the transcript and authentication-string
  helpers are private to `jet-core`, so the shell reimplements them. The live
  run below confirms they agree.
- `client_negotiated_minor_accessor`: `Client::negotiated_minor()` is
  crate-private. The shell infers bounds from status fields and typed
  refusals; below minor 37 some feature rows stay "Checked when used".
- `recent_conversations_query`: pages come oldest first, so Recent walks each
  Plane's whole chain. A remote Plane stops after 256 pages (65,536 tasks) and
  says "Newest tasks from {label} may be missing".
- `plane_display_name`: neither status nor capabilities carry a device name,
  so a remote Plane's label is its SSH address.
- A cancel-offer Command: today only closing the gate ends an offer.
- `PairedClient` has no display name or last-seen time. Rows show a key
  fingerprint, the client ID prefix and the paired date.
- Access is enabled or disabled only. There is no scoped ("limited") access.
- **New finding: re-pairing after a revoke does not survive a jetd restart.**
  Pairing a client ID again after it was revoked works until jetd restarts;
  after the restart the paired-client row is gone and the login is refused
  with `connection.unauthorized`. Revoking deletes the row and records the
  client ID in the store's deletion ledger (`delete_paired_client`,
  `jet-store/src/pairing/paired_client.rs`), and the ledger appears to remove
  the re-paired row again at startup. A client that was never revoked is not
  affected. Reproduce with `live_e2e` (step 6 prints the result). The user can
  recover with Pair again, which fails the same way after the next restart.

## Deferred client exceptions and remaining client work

- ADR-0076 "Set up secure storage" through PackageKit's session D-Bus service.
  3.1 shows manual instructions instead.
- "Runs on" for new tasks on a remote Plane, and Project setup on a remote
  Plane. New tasks and all of Setup use this computer; the composer shows
  "Runs on" truthfully. The `planeId` plumbing is ready for 3.2/3.4.
- QR offers: only manual codes are offered on Linux.
- A Security audit view of pairing events, and audit-epoch recovery (Wave 3.3).
- Swift parity: the Swift app still shows "Pairing controls arrive in Wave 3"
  (`apps/jet/jet/Features/Settings/JetSettingsView.swift`).
- Settings has a Connections summary with links into Planes. The full
  five-group Settings layout is Wave 3.2.

## Fixed latent defects

Items from the spec's list of defects in code this wave touched:

1. `build.rs` omitted six registered commands. They are listed now, the hand
   written permission files were regenerated, and a test checks that handlers,
   `build.rs` and the capability agree.
2. Feed leak: every recovery started another native poll loop. Feeds now have
   IDs and abort handles, with one live feed per Plane.
3. The live UI showed the fixture Plane name. It now shows "This computer" or
   the Plane's label.
4. Recent was oldest first. The catalog walks the chain and sorts newest first.
5. The notification gate had one cursor and fenced only the local Plane. The
   cursor is now per Plane and every Plane is fenced.
7. The timeline filter ignored the Plane. It matches Plane and Conversation.
8. Recovery actions were not bound to a Plane. `resume_events` restarts the
   feed of the Plane whose command failed.

Item 6 is left open: Setup's project and removal grants
(`SetupState.project_grants`/`removal_grants` in `setup.rs`) have no cap, and
expired entries are evicted only when used. The new pairing and enrollment
stores are bounded.

Slice 5's accessibility pass also fixed:

- The sidebar's "Open Planes" and "Retry" links on a Plane status row were
  4.08:1 in light mode; they use the stronger accent now.
- Status text ("Enabled", "Connected") and danger text buttons ("Revoke…",
  "Remove") used dark-theme colors in light mode (1.6–1.9:1). New
  `--success-text` and `--danger-text` tokens are 4.5:1 or better in both
  themes.
- Primary buttons in light mode had dark text on the accent at 4.11:1. They
  use white text (4.6:1, 5.9:1 on hover), as the Send button already did.
- The pairing code and the confirm-step authentication string put
  `aria-label` on a `<p>`, which screen readers may ignore. They now carry
  visually hidden text with the digits spelled out.
- The Add a Plane input overlapped its label because of the shared
  removal-dialog margin.

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

Not re-tested. It needs a desktop session with a notification server, a
connected remote Plane and Runs producing events on both Planes, which the
sandbox below does not have (its private D-Bus bus has no notification
server). The zbus runtime is unchanged (see above), and the automated
notification tests pass, but step 10 of the manual matrix is still open.

## Verification

Run from `apps/jet-tauri` with the pinned Rust 1.98.1 toolchain:

```sh
bun install --frozen-lockfile
bun run check
bun run test
bun run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run tauri build --debug --bundles deb
```

Results: svelte-check 0 errors and 0 warnings; Vitest 100 tests in 11 files;
Rust 95 tests passed and 2 ignored (the live matrix and the real Secret Service
probe, both run by hand below); the debug Debian bundle `Jet_0.1.0_amd64.deb`
built.

### Live end-to-end run

`scripts/remote-plane-sandbox.sh` builds a disposable remote Plane on this
machine. It runs an unprivileged sshd on 127.0.0.1 with its own host key that
accepts only a sandbox key and only `jetd connect --stdio`, puts an `ssh`
wrapper first on `PATH` that adds a sandbox `ssh_config` (so the app's fixed
options still apply and host keys are checked against a sandbox
`known_hosts`) and logs every spawn, and starts `jetd serve` for the remote
Plane and for this computer's local Plane. It also starts a private D-Bus bus
with its own GNOME Keyring and no display. Nothing in `~/.ssh`, `~/.jet` or the
login keyring is touched, and everything is deleted on exit.

`src-tauri/src/jet/live_e2e.rs` (ignored by default) drives the shell's own
command functions through that sandbox, with three bridges standing in for
three computers:

```sh
cargo build --manifest-path ../../packages/Cargo.toml -p jet-daemon --bin jetd
scripts/remote-plane-sandbox.sh ../../packages/target/debug/jetd -- \
  cargo test --manifest-path src-tauri/Cargo.toml live_e2e -- --ignored --nocapture
```

Outcome of the spec's manual matrix on 2026-09-23 (jetd built from this
branch's `packages/`, OpenSSH 10.5p1, GNOME Keyring):

| Step | Result |
|---|---|
| 1. Owner flow on a local Plane | Passed through the shell functions: gate, offer, pending claim with the claimant's client ID, confirm, gate closed. A wrong string is refused with `pairing.authentication_string_mismatch`. The GUI was not clicked through. |
| 2. Enroll, cross-implementation check, late confirm | Passed. jetd accepted the string this app computed, so the shell's transcript code agrees with `jet-core`. Confirmed 100 s after the claim and Finish succeeded. The spec asked for 3 minutes, but jetd's window is 2 minutes from the claim (`PAIRING_WINDOW_MS`); at 180 s confirm returns `pairing.offer_expired`. |
| 3. Remote owner operation | Passed. This app opened an offer on the remote Plane over SSH, and a third computer paired through it and reached the same Plane identity. |
| 4. Disable, enable, revoke, pair again | Passed. Disabled → `connection.unauthorized`, state failed, and two further requests started no ssh. Enable → Retry → online. Revoke → `connection.unauthorized`. Pair again kept the same Plane handle. Audit entries on the target were not inspected. |
| 5. Self-revoke, then Forget | Passed. Receipt `applied_unverified`, Plane marked no longer allowing this computer, Forget removed it. Add a Plane afterwards paired again. |
| 6. jetd stopped | Passed for the shell: `plane.jetd_unavailable`, state reconnecting, online again after jetd started. Recent and Search offline rows are covered by the Vitest catalog tests, not by this run. Found the re-pair-after-revoke backend defect above. |
| 7. Host key not trusted | Passed. `ssh.connection_failed`, no prompt, and `known_hosts` stayed empty. |
| 8. Locked keyring | Partly. Locked → `identity.secret_store_locked` with no ssh started. With no display the prompt could not be answered, so this is the "dismissed" case; unlocking through the prompt was not tried. After unlocking by restarting the daemon, login succeeded. |
| 9. No Secret Service | Passed. Add → `identity.secret_store_unavailable`; session-only pairing worked; a new bridge on the same app data (an app restart) → `identity.session_ended`; keyring back → Pair again stored a durable key. The Plane still held the ended session key for that client, so the owner revoked it before pairing again. The local Plane kept working throughout. |
| 10. Notifications with a remote Plane | Not verified (see above). |
| 11. Read-only or Security-degraded Plane | Not verified: jetd has no switch for these states. The paused-controls behavior is covered by the pairing Vitest cases. |
| 12. Older jetd | Not verified: no older jetd was available. Unsupported presentation is covered by the model and catalog tests and the mocked visual checks. |

All 33 ssh processes the app started used exactly the fixed argument list.
(jetd itself runs `ssh -V` once; the harness ignores spawns whose parent is
jetd.)

The real Secret Service probe (`the_real_secret_service_probe_round_trips`)
also passed against the sandbox's GNOME Keyring.

### Native and visual checks

- The debug build started under Hyprland (Wayland, with
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`) inside the sandbox and showed the local
  Plane as connected. The GUI was not driven further; no input automation that
  stays out of the user's session was available.
- The real Svelte components were audited with axe-core 4.12 at 900×600 in
  light and dark, with mocked IPC covering four Planes (connected, older Jet,
  needs pairing / session only), a pending claim, paired clients, the Add a
  Plane destination, code and confirm steps, the revoke dialog for this
  computer, Settings with Connections, and Search groups including an
  unsupported and a failed Plane. After the fixes above there are no
  violations in the Wave 3.1 surfaces.
- Three axe findings remain, all in code older than this wave: the page has no
  `main` landmark, the `kbd` shortcut hint on the active sidebar row is below
  4.5:1 in dark mode, and the composer's context row puts `aria-label` on a
  `div`. The new tokens also fixed Setup's "Connected" and "Remove" colors,
  which had the same light-mode problem.

### Not verified

- Keyring variants other than GNOME Keyring: KWallet and KeePassXC.
- Unlocking a locked keyring through its prompt (step 8), desktop
  notifications (step 10), read-only and degraded Planes (step 11), and an
  older jetd (step 12).
- Clicking through the flows in the running app; the flows were exercised
  through the shell's command functions and the UI with mocked IPC.
- A remote Plane on another machine; the sandbox runs both Planes on this
  computer over loopback SSH.
- RPM and AppImage packaging, and macOS.
