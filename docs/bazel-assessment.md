# Bazel assessment

Research date: 2026-09-08. Investigation only; no build-system implementation or performance benchmark was performed.

## Recommendation for this repository

Defer a repository-wide Bazel migration. Jet has a credible future use for Bazel, especially shared build caching and a dependency graph connecting protocol generation to clients and release packages. The current evidence favors improving Cargo development settings, test prerequisites, and CI first. This is a prioritization judgment, not a claim that Bazel cannot support Jet or that a small Rust workspace can never benefit from it.

The existing [issue #106, “Integrate Bazel”](https://github.com/apexgang/jet/issues/106), has no body, comments, or stated success criteria as of this assessment. It should acquire a measurable motivation before becoming an implementation project. Neither that issue nor any other tracker item was modified.

## Repository evidence

The inspected checkout was at `d9783ba`. Counts below cover tracked files, include tests and scaffolding, and are not build timings.

| Area | Observed state | Implication for Bazel |
| --- | --- | --- |
| Rust backend | [Workspace](../packages/Cargo.toml) has 11 members, including the fuzz package, with centralized dependencies. About 85,500 source lines including tests and generated contract helpers; 39 direct integration-test entry files. | Substantial backend, but still a manageable crate graph. A migration has meaningful test integration work. |
| Native GUI | [Xcode project](../apps/jet/jet.xcodeproj/project.pbxproj) has app, unit-test, and UI-test targets. Eight Swift source files total 2,711 lines, of which 2,472 are generated models. [ContentView](../apps/jet/jet/ContentView.swift) remains a scaffold. | There is little existing modular Swift build work to accelerate today. |
| Tauri GUI | [Package manifest](../apps/jet-tauri/package.json) uses SvelteKit, Vite, TypeScript, and Bun; [Tauri configuration](../apps/jet-tauri/src-tauri/tauri.conf.json) invokes Bun. About 247 non-generated source/configuration lines; the [page](../apps/jet-tauri/src/routes/+page.svelte) still presents the template greeting. Its Rust package is outside the backend workspace. | Multiple tools exist, but the implemented frontend graph remains small. |
| Generated contracts | [Just recipes](../packages/justfile) compile Rust schema emitters, run [Python generation](../scripts/wire-models.py), and check fixtures in Rust, TypeScript, and Swift. | This is the clearest current seam where one declared build graph could improve dependency tracking. |
| Native and compile-time inputs | [Cargo.lock](../packages/Cargo.lock) contains 303 package records, including native build dependencies; not all records necessarily compile on a given platform. SQLx uses 138 committed query-metadata files and embedded migrations. | Cargo import alone is insufficient: metadata, migrations, native tools, and build-script behavior need explicit handling. |
| CI | No tracked GitHub Actions or other conventional CI configuration was found; `gh run list` returned no runs. [Issue #59's latest comment](https://github.com/apexgang/jet/issues/59#issuecomment-5581811661) confirms that contract checks are not connected to CI and the Swift fixture runner still needs verification. | There is an immediate correctness gap that the existing commands can address. There is no measured CI bottleneck here to justify a cache migration yet. |

[ADR-0050](adr/0050-connect-every-gui-through-jetd.md) deliberately connects Swift to `jetd` through a wire protocol instead of Rust FFI. This reduces direct Rust/Swift compilation coupling. It specifies future Tauri reuse of `jet-client`; the current Tauri manifest does not yet declare that dependency. [ADR-0032](adr/0032-limit-v1-platforms-to-macos-ios-and-linux.md) limits v1 to macOS, iOS, and Linux, so Windows does not need to enlarge an initial evaluation.

Coordinated releases are a real future motivation: [ADR-0053](adr/0053-version-bundled-products-together-and-protocols-separately.md) versions the bundled products together, while [issue #57](https://github.com/apexgang/jet/issues/57) and [issue #58](https://github.com/apexgang/jet/issues/58) still cover packaging and release qualification. Bazel could organize dependencies and cache unsigned build outputs; it would not itself implement installation, signing, notarization, rollback, or release acceptance checks.

## Improvements to evaluate first

1. **Measure and revise development build settings.** [Cargo.toml](../packages/Cargo.toml) sets `opt-level = "z"`, `lto = true`, and `codegen-units = 1` in the development profile. Cargo documents that full LTO adds link time, more codegen units permit parallel compilation, and the test profile inherits development settings. These make the profile a strong candidate for investigation, not a proven bottleneck. Benchmark development-oriented alternatives before changing compilers or build systems. Keep release optimization decisions separate. [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
2. **Scope helper prerequisites.** Every `just test`, including a package-scoped invocation, first invokes builds for `jet-fueld` and both bundled Crafts. Cargo may find them fresh; this is not a claim that they always recompile. Still, protocol-only or store-only tests should not need unrelated helper preparation. Investigate making those prerequisites specific to the subprocess suites that use them. [Test recipes](../packages/justfile)
3. **Add a repeatable CI baseline using the existing commands.** Pin required tools and run Rust checks, SQLx metadata/migration checks, contract drift checks, and language fixtures on the appropriate platforms. The backend already pins Rust and commits a Cargo lockfile; the frontend commits `bun.lock`. Connect the checks before treating Bazel adoption as a prerequisite for correctness. [Backend toolchain](../packages/rust-toolchain.toml), [SQLx configuration](../packages/.cargo/config.toml), [contract documentation](wire-contracts.md), [issue #59](https://github.com/apexgang/jet/issues/59)
4. **Evaluate caching against the observed workload.** Retained Cargo outputs and dependency caches are a baseline; sccache is an optional comparison with the limitations below. Record cold builds, warm builds, individual edits, and clean worktree builds. Use Cargo's timing reports to locate compilation and linking costs before attributing waits to the build orchestrator. [Cargo timing reports](https://doc.rust-lang.org/cargo/reference/timings.html)

The current release profile also applies full LTO and size optimization uniformly, whereas [ADR-0059](adr/0059-optimize-release-binaries-by-role.md) calls for different optimization policies for `jetd` and the smaller helpers. This is an existing profile/decision mismatch to resolve during packaging; a Bazel migration would need to preserve the intended role-specific behavior rather than copy the current settings blindly.

## Concrete migration work

The Rust side needs more than generated external dependencies. Examples include [SQLx migration embedding](../packages/jet-store/src/migrations.rs), the [migration-tracking build script](../packages/jet-store/build.rs), SQLx offline environment and query metadata, schema-only features, and explicit runtime fixtures. [Utility integration tests](../packages/jet-daemon/tests/utility.rs) locate Craft binaries by walking up from their own executable; other tests use Cargo binary environment variables. These assumptions need mapping to Bazel runtime inputs. [No-Visa tests](../packages/jet-runtime/tests/no_visa.rs) deliberately exercise process groups through [the runtime implementation](../packages/jet-runtime/src/process/no_visa.rs), so the suitability of Bazel's normal test execution contract must be checked instead of assuming all tests can be cached or run remotely unchanged.

[ADR-0046](adr/0046-align-rust-crates-with-deployment-and-stability-seams.md) intentionally keeps deep crates at deployment and stability seams. Do not split them into feature-sized crates merely to create more Bazel targets; that would reopen an explicit architecture decision. Bazel can reuse and schedule crate actions, but adding build metadata does not change those compilation boundaries.

## Current ecosystem findings

### Rust and Cargo

`rules_rust` is a viable way to build Rust with Bazel. Its `crate_universe` extension can read a Cargo workspace's `Cargo.toml` and `Cargo.lock`, generate external dependency targets, and expose `all_crate_deps`, `aliases`, and `crate_edition` so Cargo manifests remain the source of dependency declarations. First-party library, binary, and test targets still need Bazel definitions; dependency changes introduce a Bazel repinning workflow. Thus adoption need not mean manually duplicating every crates.io dependency, but importing Cargo dependencies is not the complete migration. [Crate Universe documentation](https://bazelbuild.github.io/rules_rust/crate_universe_bzlmod.html)

The compilation unit remains a Rust crate: `rust_library` passes the crate root and source files to `rustc`. Adding Bazel does not automatically make individual Rust modules independently buildable. Smaller compilation boundaries would require crate changes. This is an inference from the documented rule model, not a measured limitation on Jet's build speed. [Rust library rule](https://bazelbuild.github.io/rules_rust/rust_library.html)

Cargo build scripts are supported through `cargo_build_script`, including declared input data, tools, environment variables, and the C/C++ toolchain. This makes bundled native dependencies such as SQLite plausible, rather than an automatic blocker. Their actual builds still need validation on Jet's supported platforms. Offline SQLx metadata, migrations, and generated inputs must be declared at the appropriate compilation or build-script boundary. [Cargo build-script rule](https://bazelbuild.github.io/rules_rust/cargo_build_script.html)

Compiler configuration must be compared explicitly: `rules_rust` documents separate optimization defaults (`dbg` and `fastbuild`: `0`; `opt`: `3`) and an LTO setting. Do not assume a new Bazel build reproduces Jet's Cargo profiles, or credit Bazel for speedups caused by reducing optimization. [Rust toolchain](https://bazelbuild.github.io/rules_rust/rust_toolchain.html), [Rust settings](https://bazelbuild.github.io/rules_rust/rust_settings.html)

`rust_test` normally uses Rust's libtest harness, with separate targets for integration-test crates. Jet now uses Cargo's built-in test runner through `just test`. A Bazel migration would still need to preserve package and test-name selection, doctest coverage, and subprocess fixtures. [Rust test rule](https://bazelbuild.github.io/rules_rust/rust_test.html)

Bazel's test contract requires declared runtime inputs accessed through runfiles and explicitly disallows deriving other build outputs' paths from a test executable's location. It also places constraints on process/session handling. Jet's process-oriented integration tests and Cargo-layout helper discovery are consequently a concrete migration area; some tests may need tailored execution settings. [Bazel test encyclopedia](https://bazel.build/reference/test-encyclopedia)

### Native Apple client

The relevant stack exists: `rules_swift` compiles Swift; `rules_apple` links and bundles Apple applications; `rules_xcodeproj` generates Xcode projects from Bazel targets. The latter documents indexing, debugging, test selection, and Xcode Previews. It would be incorrect to claim Bazel requires abandoning Xcode or cannot support SwiftUI previews. [Swift rules](https://github.com/bazelbuild/rules_swift), [Apple rules](https://github.com/bazelbuild/rules_apple), [Xcode project rules](https://github.com/MobileNativeFoundation/rules_xcodeproj)

This introduces another maintained set of build definitions and compatibility relationships. The Swift and Apple rule maintainers explicitly warn that Bazel upgrades commonly require rule upgrades; `rules_xcodeproj` publishes its own compatibility matrix. Apple development still needs the Xcode toolchain. Compatibility with Jet's exact installed Xcode/macOS combination should be checked against the chosen releases rather than inferred from generic support. [Swift setup and supported versions](https://github.com/bazelbuild/rules_swift), [Apple supported versions](https://github.com/bazelbuild/rules_apple), [Xcode project compatibility](https://github.com/MobileNativeFoundation/rules_xcodeproj#compatibility)

### SvelteKit, Vite, Bun, and Tauri

Aspect's established `rules_js` is built around Node.js tools and pnpm-style dependency management. It can run JavaScript tooling and provides a development-server wrapper, but adopting it is a dependency/runtime integration decision for a Bun project. [JavaScript rules](https://github.com/aspect-build/rules_js)

Bun-native third-party rules do exist, including `bun.lock` consumption, so describing Bun as categorically unsupported would be inaccurate. The researched `rules_bun` project documents both hermetic operations and an intentionally non-hermetic development runner, with its own platform coverage. Its existence does not establish that Jet's SvelteKit/Vite/Tauri workflow can migrate unchanged; that combination needs validation and maintenance ownership. [Bun rules and stated scope](https://github.com/tomato-bazel/rules_bun)

Tauri already coordinates the frontend build through `beforeBuildCommand` and consumes its output through `frontendDist`. A Bazel migration would need to preserve that relationship, plus the Rust build and packaging. A wrapper around the whole existing Tauri command may be useful orchestration but would expose only a coarse action unless its constituent work is modeled separately. The latter is an architectural inference from Tauri's documented pipeline. [Tauri Vite integration](https://v2.tauri.app/start/frontend/vite/), [Tauri configuration](https://v2.tauri.app/reference/config/)

### Caching and expected benefits

Bazel caches action results using declared inputs, commands, environments, and output metadata, and can share outputs among developers and CI through a remote cache. For Jet, its strongest prospective advantage is one dependency graph spanning Rust-generated schemas, Python model generation, both GUI clients, and packaging, plus reusable results across machines. Correct input declarations and reproducible toolchains are prerequisites. Remote cache operation also has bandwidth, storage, and maintenance costs. No speedup magnitude is established by this analysis. [Remote caching](https://bazel.build/remote/caching)

`sccache` is a smaller-scope alternative for eligible Rust compilation. Its documentation requires disabling Rust incremental compilation and excludes crates that invoke the system linker, including binaries, dynamic libraries, and procedural macros. Consequently it cannot be presented as equivalent to Bazel's general action cache or as a guaranteed remedy for link-heavy builds. Benchmark it separately with an appropriate profile. [sccache Rust limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)

Useful evaluation cases are a cold build, unchanged warm build, small Rust implementation edit, protocol edit with regeneration, dependency update, and a fresh checkout consuming a populated remote cache. Hold compiler versions, optimization settings, target platforms, and test semantics constant. Compare against an improved Cargo baseline, not just the current settings. These are proposed measurements, not results.

## When to reconsider

Reconsider if many developers or parallel agent worktrees repeatedly compile the same dependencies and binaries; CI repeatedly rebuilds unchanged work despite ordinary caching; generated outputs and packaging gain enough dependencies to be difficult to keep correct; or hermetic builds become an explicit product requirement. Team size, repeated-build frequency, cache hit rates, and actual build latency were not established here. These may materially change the recommendation. Native outputs still depend on target platform, architecture, compiler, and SDK; do not count Linux and macOS builds as interchangeable cache entries.

A future evaluation should start with the protocol → schemas → generated models → fixture-tests path, then include a representative `jetd`/SQLx build and a helper subprocess test before declaring success. A protocol-only demonstration would miss the hardest Rust integration costs. Keep native Xcode and Tauri development available during evaluation, and delay full GUI migration until its value is demonstrated. A wrapper that simply invokes the whole existing Cargo or Tauri build is not evidence of fine-grained caching benefits.

Compare the same target and compiler settings against an improved Cargo baseline on macOS and Linux. Record total wall time, repeated build reuse, test parity, clean worktree behavior, and maintenance effort. Choose a material improvement threshold appropriate to the actual workload before evaluating results. No implementation, benchmark, issue edit, commit, or test run was performed in this assessment; the only added file is this research note.
