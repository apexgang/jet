# Contributing to Jet

Thanks for contributing to Jet. This guide covers the common path for code and
documentation changes.

## Before you start

1. Search the [issue tracker](https://github.com/apexgang/jet/issues) for an
   existing report or proposal.
2. For a substantial behavior or architecture change, open an issue before
   implementation so the design can be agreed on first.
3. Read the root [`AGENTS.md`](../AGENTS.md). For backend changes, also read
   [`packages/AGENTS.md`](../packages/AGENTS.md) and the relevant design documents.

Do not use a public issue to report a vulnerability. Follow
[`SECURITY.md`](SECURITY.md) instead.

## Development setup

The core supports macOS and Linux. Install these tools:

- Git
- [Rustup](https://rustup.rs/)
- [`just`](https://just.systems/)

The repository pins Rust 1.98.1 in `packages/rust-toolchain.toml`. Node.js 24 or
newer is required for the TypeScript contract tests. Swift is required for the
Swift contract tests.

```sh
git clone https://github.com/apexgang/jet.git
cd jet/packages
just install
just test
```

Run every backend command from `packages/`. Use `just --list` to see the available
recipes.

## Make a change

1. Create a focused branch from the current `main` branch.
2. Keep terminology consistent with [`CONTEXT.md`](../CONTEXT.md) and preserve
   decisions recorded under [`docs/adr`](../docs/adr/).
3. Add or update tests for behavior changes. Keep public wire changes compatible
   with the rules in [`docs/wire-contracts.md`](../docs/wire-contracts.md).
4. Update documentation when behavior, commands, configuration, or operational
   expectations change.
5. Open a pull request using the repository template and link the relevant issue.

The Rust workspace forbids unsafe code. Keep dependencies in the workspace
`Cargo.toml`, keep crate interfaces narrow, and follow the module and test rules in
`packages/AGENTS.md`.

## Validate the change

Run the smallest relevant checks while developing. Before submitting a backend
change, format the workspace and run the affected crate's tests:

```sh
cd packages
just fmt
just clippy -p jet-core -- -D warnings
just test -p jet-core
```

Replace `jet-core` with the crate you changed. Changes to shared, core, or protocol
code may require the full `just test` suite. Wire changes also require:

```sh
just contracts
just contracts-check
just contracts-test
```

Do not commit generated contract changes without the source DTO change that
produced them.

## Pull requests

Keep each pull request to one coherent change. In its description, explain the
observable result, the reason for the change, the checks you ran, and any known
compatibility or security impact. Reviewers may ask for a smaller pull request if
unrelated work makes the change hard to verify.

By submitting a contribution, you agree that it is licensed under the repository's
[Apache License 2.0](../LICENSE).
