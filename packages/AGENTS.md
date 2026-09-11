# Jet

Jet is a multi-platform app for running and managing AI coding agent conversations across local and remote machines.

## Naming conventions & coding standards

In the packages directory where the Jet backend Rust code lives:

- All crates sit directly under the `packages` directory.
- Crate names start with `jet-`, and each crate sits in a folder named exactly like the crate. For example, `packages/jet-core` holds the `jet-core` crate.
- Keep all project dependencies in the workspace's `Cargo.toml`. Reference workspace dependencies inside crates.
- When using `format!` and you can inline variables into `{}`, always do that.
- Always collapse `if` statements per https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if.
- Always inline `format!` args when possible per https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args.
- Use method references over closures when possible per https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls.
- Avoid bool or ambiguous `Option` parameters that force callers to write hard-to-read code such as `foo(false)` or `bar(None)`. Prefer enums, named methods, newtypes, or other idiomatic Rust API shapes when they keep the callsite self-documenting.
- When possible, make `match` statements exhaustive and avoid wildcard arms.
- Newly added traits should include doc comments that explain their role and how implementations are expected to use them.
- Discourage both `#[async_trait]` and `#[allow(async_fn_in_trait)]` in Rust traits.
  - Prefer native RPITIT trait methods with explicit `Send` bounds on the returned future.
  - Preferred trait shape: `fn foo(&self, ...) -> impl std::future::Future<Output = T> + Send;`
  - Implementations may still use `async fn foo(&self, ...) -> T` when they satisfy that contract.
  - Do not use `#[allow(async_fn_in_trait)]` as a shortcut around spelling the future contract explicitly.
- When writing tests, prefer comparing the equality of entire objects over fields one by one.
- Do not add tests for values that are statically defined.
- Do not add negative tests for logic that was removed.
- Prefer private modules and explicitly exported public crate API.
- Group related implementation under feature directories such as `src/pairing/` or `src/connection/`. Use `mod.rs` for the feature's interface and declarations, with focused submodules for its operations, types, and conversions. Keep the existing crate boundaries from ADR-0046.
- Keep `lib.rs` and `main.rs` focused on crate exports and executable setup. Import internal implementation through its owning feature instead of adding aliases for old module paths at the crate root.
- Avoid large modules:
  - Prefer adding new modules instead of growing existing ones.
  - Target Rust modules under 500 LoC, excluding tests.
  - If a file exceeds roughly 800 LoC, add new functionality in a new module instead of extending the existing file unless there is a strong documented reason not to.
  - When extracting code from a large module, move the related tests and module/type docs toward the new implementation so the invariants stay close to the code that owns them.
- When running Rust commands (e.g. `just fix` or `just test`), be patient with the command and never try to kill them using the PID. Rust lock contention can make execution slow; this is expected.

Run `just fmt` (in the `packages` directory) automatically after you have finished making code changes anywhere in this repository; do not ask for approval to run it. Additionally, run the tests:

1. Use `just test` to run unit tests, integration tests, and doctests through Cargo's built-in test runner. Keep test command defaults in the justfile.
2. Run the tests for the specific project that was changed. For example, if changes were made in `packages/jet-store`, run `just test -p jet-store`. Use `just test-doc -p jet-store` when only doctests need checking.
3. Once those pass, if any changes were made in common, core, or protocol, run the complete test suite with `just test`. Avoid `--all-features` for routine local runs because it expands the build matrix and can significantly increase `target/` disk usage; use it only when you specifically need full feature coverage. Project-specific or individual tests can be run without asking the user, but do ask the user before running the complete test suite.
4. Use the development Cargo profiles selected by the justfile for checks and tests. Never use the release profile to validate changes.

Before initializing a large change to `packages`, run `just fix -p <project>` (in the `packages` directory) to fix any linter issues in the code. Prefer scoping with `-p` to avoid slow workspace-wide Clippy builds; only run `just fix` without `-p` if you changed shared crates. Do not re-run tests after running `fix` or `fmt`.

## Database

`jet-store` is the only crate that touches SQLite, and it does so through `sqlx`.

- Always run SQLx operations through the recipes in `packages/justfile`, from `packages/`. If an operation is missing, add a recipe before running it so its defaults remain centralized.
- Queries use the compile-time checked macros (`sqlx::query!`, `sqlx::query_scalar!`, `sqlx::query_as!`). They take a string literal, so SQL cannot be assembled with `format!`. Three kinds of statement are exempt because the macros cannot describe them: pragmas and `VACUUM INTO`; a `MATCH` against the FTS5 `search_documents` table, which crashes the macro's type inference in sqlx 0.9.0; and the bootstrap reads of `sqlite_master` and `_sqlx_migrations` that `Store::open` runs before the schema exists. Those run on the runtime API with a comment naming the reason.
- Those macros build from `jet-store/.sqlx`, which is committed. Regenerate it with `just sqlx-prepare` in the same commit as any SQL or migration edit, and gate on `just sqlx-check`. Nothing else notices a stale cache. `packages/.cargo/config.toml` pins `SQLX_OFFLINE=true` so a `DATABASE_URL` exported for an unrelated project cannot hijack the build.
- Add migrations with `just sqlx-migrate-add <name>`. The recipe explicitly selects timestamp versioning and simple forward-only migrations: SQLx otherwise infers numbering and reversibility from existing files. Versions have one-second resolution, so run `just sqlx-migrations-check` after a merge to catch duplicate versions and invalid migration conventions. Cache preparation and validation also run this check.
- Never write `-- no-transaction` in a migration. It cannot roll back, so a failure leaves the store half-migrated with no bookkeeping row and every later start fails on the object it already created.
- SQLite reports a bare `TEXT PRIMARY KEY` column as nullable. Tell the macro otherwise with `AS "col!"` instead of changing the schema.
- Keep the `sqlite-bundled` feature and never set `LIBSQLITE3_SYS_USE_PKG_CONFIG`. Either one silently links the distribution's SQLite, which may lack the FTS5 that ADR-0057 requires.
- `sqlx-cli` 0.9.0 is a separately installed developer tool with default features disabled and `sqlite,rustls` enabled; keep it out of workspace dependencies.

## Wire contracts

`jet-protocol` is the source of both wire contracts, and the GUIs compile
models generated from them.

- Run `just contracts` in the same commit as any change to a wire DTO, and
  gate on `just contracts-check`. Nothing else notices a stale contract; the
  committed schema and the app models are what clients build against.
- A field using `serde(with)` must also name its schema stand-in
  (`crate::Decimal`, `crate::OptionalDecimal`, `crate::Hex<N>`), because
  schemars sees the Rust type rather than the wire form.
- Cover a new message shape in `contracts/jet-fixtures.json` or
  `contracts/craft-fixtures.json`, which Rust, Swift, and TypeScript all read.
  See [Jet wire contracts](../docs/wire-contracts.md).

## Code review rules

### Crate API surface

Keep crate API surfaces as small as possible. Avoid proliferating test-only helpers.

### Breaking changes

Search for breaking changes in external integration surfaces:

- Wiki static website serving
- CLI parameters
- configuration loading

### Change size guidance (800 lines)

Unless the change is mechanical, the total number of changed lines should not exceed 800 lines. For complex logic changes, the size should be under 500 lines.

If the change is larger, explore whether it can be split into reviewable stages and identify the smallest coherent stage to land first. Base the staging suggestion on the actual diff, dependencies, and affected call sites.

## Commands

Always use [justfile](./justfile) for backend commands. Run recipes from `packages/`, or use `just --justfile packages/justfile <recipe>` from the repository root. Use `just --list` to discover recipes; add a missing operation to the justfile before using it.

`just test` forwards Cargo arguments, including `-p <crate>`, `--test <target>`, and a test-name filter. Pass libtest options after `--`, for example `just test -p jet-core pairing -- --nocapture`. Use `just test-list -p <crate>` to inspect the selection. Tests within a target share a process, so fixtures must isolate their files, sockets, and other resources. `just test-doc` forwards Cargo doctest arguments.

## Tests

### Test module organization

Keep unit tests in an inline `#[cfg(test)] mod tests` at the end of the file containing the code they exercise. Integration tests belong in the crate's `tests/` directory and exercise its public interface or deployed executables.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Tests of this module's behavior.
}
```

- Move related unit tests with extracted implementation. Tests are excluded from the 500-line target and 800-line threshold for implementation modules.
- Shared fixture code may live in a `#[cfg(test)]` support module. Keep assertions and test cases with the implementation they cover, and integration fixtures under `tests/support/`.

### Benchmarks

Cargo benchmarks can be run with `just bench`; use the divan crate to write new ones.

Use `just bench-smoke` to dry-run the benchmark for a single iteration to ensure it works.

### Test assertions

- Tests should use `pretty_assertions::assert_eq` for clearer diffs. Import this at the top of the test module if it isn't already.
- Prefer deep equality comparisons whenever possible. Perform `assert_eq!()` on entire objects rather than individual fields.
- Avoid mutating process environment in tests; prefer passing environment-derived flags or dependencies from above.

## Platform support

Tests and features must support Linux and macOS unless a feature is explicitly OS-specific.
