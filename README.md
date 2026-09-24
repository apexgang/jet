# Jet

Jet runs AI coding Conversations across local and remote computers from one
desktop app. It gives Codex and Claude Code the same project, run, terminal,
approval, and history model without hiding the native Harness underneath.

Each Conversation has one authoritative Home Plane. A Plane is a computer
running `jetd`. Jet can run a Harness on that Plane, run it natively on a paired
remote Plane, or keep the Harness local while it works through controlled remote
tools.

> [!WARNING]
> Jet is under active development. The core targets macOS and Linux. The native
> Apple and Tauri clients are still being built, and the v1 release contract has
> open gates.

## What Jet manages

- Durable Conversations with queued turns, schedules, approvals, and retained
  history.
- Isolated Git Workspaces so concurrent Conversations do not overwrite one
  another.
- Local and paired remote Planes, with explicit authority and recovery rules.
- Codex and Claude Code through versioned Jet Crafts instead of GUI-specific
  integrations.
- Run recovery through `jetfueld`, which keeps an execution alive while `jetd`
  restarts or upgrades.

## Architecture

```text
SwiftUI client             Tauri client
       \                       /
        +---- Jet protocol ---+
                   |
                 jetd
          authoritative state
             /            \
        jetfueld       Jet Crafts
     run continuity    /         \
                  Codex CLI   Claude Code
```

Every GUI uses the same versioned protocol. `jetd` owns the state for its Plane,
Jet Crafts translate Harness-native events and commands, and `jetfueld` keeps
active work alive if the daemon is temporarily unavailable.

The [domain glossary](CONTEXT.md) defines Conversation, Plane, Workspace, Run,
Harness, Craft, and the other terms used throughout the repository.

## Repository

| Path | Contents |
| --- | --- |
| [`packages/`](packages/) | Rust workspace for `jetd`, `jetfueld`, protocols, storage, clients, and bundled Crafts |
| [`apps/jet/`](apps/jet/) | Native SwiftUI client for Apple platforms |
| [`apps/jet-tauri/`](apps/jet-tauri/) | Cross-platform Tauri and Svelte client |
| [`docs/`](docs/) | Behavior, operations, recovery, protocol, and architecture records |

## Install the core

On Linux, Homebrew installs the compiled core executables:

```sh
brew install apexgang/tap/jetd
brew services start apexgang/tap/jetd
```

On macOS, the macOS app's releases publish the core as the
`apexgang/tap/jet` formula instead.

The `jet-app` cask installs the Linux desktop app with the daemon. Name
both, because Homebrew trusts only the tap items named on the command line:

```sh
brew install apexgang/tap/jetd apexgang/tap/jet-app
```

The macOS GUI client has a separate distribution lifecycle. See
[core distribution](docs/core-distribution.md) for the packaged executables,
the Linux desktop app, supported targets, upgrades, and rollback behavior.

## Build from source

Install Git, [Rustup](https://rustup.rs/), and
[`just`](https://just.systems/). The repository pins its Rust toolchain.

```sh
git clone https://github.com/apexgang/jet.git
cd jet/packages
just install
just build
just test
```

Run `just --list` from `packages/` to see the supported development commands.
Wire-contract checks also require Node.js 24 or newer and Swift.

## Project documentation

- [Core v1 release contract](docs/release-contract.md)
- [Harness conformance matrix](docs/conformance-matrix.md)
- [Remote connections](docs/remote-connections.md)
- [Visa execution](docs/visa-runs.md) and
  [No-Visa execution](docs/no-visa-execution.md)
- [Recovery](docs/recovery.md)

Read the [contribution guide](.github/CONTRIBUTING.md) before opening a pull
request. Report vulnerabilities through the
[security policy](.github/SECURITY.md), not a public issue. Everyone
participating in the project must follow the
[code of conduct](.github/CODE_OF_CONDUCT.md).

## License

Jet is licensed under the [Apache License 2.0](LICENSE).
