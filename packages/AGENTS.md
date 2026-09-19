# Jet backend

Rust workspace for the Jet backend. Run backend commands from `packages/`.

## Structure

- Crates live directly under `packages/` and are named `jet-*`.
- Keep dependencies in the workspace `Cargo.toml`; crates reference workspace dependencies.
- Preserve the existing crate boundaries.
- Prefer private modules and explicitly exported public APIs.
- Keep crate API surfaces small.
- Organize substantial functionality by feature (`src/pairing/`, `src/connection/`, etc.), with focused submodules beneath it.
- Keep `lib.rs` and `main.rs` focused on exports, wiring, and executable setup.
- Prefer adding a focused module over growing an already large one.

Before introducing a new crate or abstraction, check whether the functionality belongs in an existing feature or crate.

## Rust

Follow existing patterns and write idiomatic stable Rust.

- Prefer simple, explicit APIs over unnecessary abstraction.
- Avoid ambiguous `bool` and `Option` parameters when an enum, newtype, or named method makes the call site clearer.
- Prefer exhaustive `match` statements.
- Inline `format!` arguments and use method references where idiomatic.
- New traits must document their role and expected implementations.
- For async traits, prefer native RPITIT with an explicit `Send` future contract; do not use `async_trait` or `allow(async_fn_in_trait)` as a shortcut.
- Do not introduce compatibility aliases for old internal module paths unless required by a public contract.

Let `just fix`/Clippy handle mechanical style rules rather than reasoning about them manually.

## Working style

Keep changes focused.

- Inspect the relevant implementation before editing.
- Follow existing patterns unless the task requires changing them.
- Prefer targeted searches and relevant files over broad repository exploration.
- Do not refactor unrelated code.
- Avoid speculative abstractions.
- Prefer compiler and test feedback over speculative reasoning.
- When a task crosses crate boundaries, inspect the affected public interfaces before changing them.

## Commands

Use `packages/justfile` for backend operations. Use `just --list` when you need to discover a command.

After code changes:

1. Run `just fmt`.
2. Run the narrowest relevant `just fix -p <crate>`.
3. Run targeted tests with `just test -p <crate>`.

Run the full `just test` when changes affect shared/core/protocol behavior or when explicitly requested. Do not use `--all-features` routinely.

Rust commands may wait on Cargo locks; do not kill them merely because they are slow.

## Tests

- Keep unit tests in the implementation file's `#[cfg(test)] mod tests`.
- Put public-interface and executable tests under the crate's `tests/`.
- Prefer equality of complete objects over field-by-field assertions.
- Use `pretty_assertions::assert_eq` where applicable.
- Do not test statically defined values or behavior that no longer exists.
- Avoid mutating process environment in tests.
- Tests and features support Linux and macOS unless explicitly OS-specific.

## Scoped rules

### Database

`jet-store` exclusively owns SQLite through SQLx.

Before changing SQL, migrations, SQLx metadata, SQLite configuration, or store initialization, read the database guidance in the relevant `jet-store` documentation/AGENTS file.

### Wire contracts

`jet-protocol` is the source of Jet wire contracts.

Before changing wire DTOs, schemas, generated client models, or contract fixtures, read the wire-contract documentation and run the contract-specific `just` recipes.

### Releases and CI

Release packaging, size budgets, CI, GitHub Actions, and release-contract rules are owned by `.github/` documentation. Read those instructions only when working in those areas.

## Completion

Before finishing:

- review the diff for accidental or unrelated changes;
- ensure relevant formatting, linting, and targeted tests pass;
- mention what changed and which checks were run.

Keep the final report concise unless more detail is requested.
