# Jet

Jet is a multi-platform app for running and managing AI coding Harness
conversations across local and remote computers.

Jet keeps each Conversation's authoritative state on one Home Plane. GUI clients
connect to the Plane's `jetd` daemon, while Jet Crafts adapt Codex and Claude Code
to the same versioned protocol. Work can run on the Home Plane or on a paired
destination Plane.

Jet is under active development. The core targets macOS and Linux. The repository
also contains a native Apple client and a cross-platform Tauri client.

## Repository layout

- [`packages`](packages/) contains the Rust workspace, including `jetd`,
  `jetfueld`, the protocol, the store, the client library, and bundled Crafts.
- [`apps/jet`](apps/jet/) contains the SwiftUI client.
- [`apps/jet-tauri`](apps/jet-tauri/) contains the Tauri and Svelte client.
- [`docs`](docs/) contains the design, protocol, operations, and recovery
  documentation.

The [domain glossary](CONTEXT.md) defines terms such as Conversation, Plane,
Workspace, Run, Harness, and Craft.

## Install the core

Homebrew installs the compiled core executables on macOS:

```sh
brew install apexgang/tap/jet
brew services start apexgang/tap/jet
```

See [core distribution](docs/core-distribution.md) for release contents,
platforms, version management, and rollback behavior.

## Build from source

Development requires Git, [Rustup](https://rustup.rs/), and
[`just`](https://just.systems/). The repository pins its Rust toolchain.

```sh
git clone https://github.com/apexgang/jet.git
cd jet/packages
just install
just build
just test
```

Run `just --list` in `packages/` to see the supported development commands. The
wire-contract checks also require Node.js 24 or newer and Swift.

## Contributing and security

Read the [contribution guide](.github/CONTRIBUTING.md) before opening a pull
request. Report vulnerabilities through the process in the
[security policy](.github/SECURITY.md). Everyone participating in the project
must follow the [code of conduct](.github/CODE_OF_CONDUCT.md).

## License

Jet is licensed under the [Apache License 2.0](LICENSE).
