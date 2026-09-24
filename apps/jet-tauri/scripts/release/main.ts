/**
 * Release checks for the Linux desktop bundles, run through `just`:
 *
 *   bun scripts/release/main.ts version-check
 *   bun scripts/release/main.ts app-version
 *   bun scripts/release/main.ts bundle-mode
 *   bun scripts/release/main.ts check-payload <jet-core-<v>-<target>.tar.gz>
 *   bun scripts/release/main.ts clean-bundles
 *   bun scripts/release/main.ts rustflags
 *   bun scripts/release/main.ts bundles [--in <directory>]
 *   bun scripts/release/main.ts verify [--payload <archive>] [--release] [--require-signatures]
 *   bun scripts/release/main.ts check-signatures <directory>
 *
 * `version-check` fails unless tauri.conf.json, src-tauri/Cargo.toml (and its
 * Cargo.lock entry) and package.json carry the core workspace version
 * (ADR-0053); `app-version` prints that version. `bundle-mode` prints
 * `release` or `unsigned` from `JET_RELEASE_SIGN`. `check-payload` validates
 * the core payload archive before a long build. `clean-bundles` removes
 * earlier deb, rpm and AppImage output so no stale bundle or `.sig` survives
 * into a new build. `rustflags` prints the `CARGO_ENCODED_RUSTFLAGS` that
 * remap build paths out of the app binary. `bundles` prints the path of this
 * machine's deb, rpm and AppImage, one per line, for `just release-sign`:
 * from the build output, or from a directory that holds only those three
 * (and their `.sig` files), as the CI signing job receives them.
 * `verify` checks them: expected names, the payload byte-identical at
 * `usr/lib/Jet/jet-core.tar.gz`, the bundle type patched into each app
 * binary, no build paths in it and the updater endpoint only in a release
 * build (`--release`, or signed), no source maps or fixtures, clean webview
 * assets, and updater signatures that verify against `plugins.updater.pubkey`
 * (all three or none; `--require-signatures` makes none a failure and implies
 * `--release`). Without `--payload` it compares against the copy in
 * `src-tauri/resources/`. It prints each bundle's size and SHA-256.
 * `check-signatures` verifies only the updater signatures of the bundles in
 * a directory `bundles --in` accepts, without opening the bundles, the build
 * or the payload. Relative paths resolve against the working directory.
 *
 * Exit status: 0 passed, 1 a check failed, 2 the checks could not run.
 */

import { spawnSync } from "node:child_process";
import {
  existsSync,
  lstatSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { debEntries, rpmEntries, type ArchiveEntry, type EntryKind } from "./archives";
import {
  binaryBuildPathFindings,
  bundleDirectoryFindings,
  cargoLockVersion,
  expectedBundles,
  formatSize,
  linuxArch,
  payloadEntryPath,
  payloadFindings,
  releaseRustflags,
  sha256,
  signatureState,
  targetTriple,
  tomlString,
  bundleEntryFindings,
  bundleMode,
  bundleTypeFindings,
  updaterEndpointFindings,
  verifyOptions,
  versionFindings,
  webviewAssetFindings,
  type DirectoryEntry,
  type ExpectedBundle,
  type LinuxArch,
  type VerifyOptions,
} from "./checks";
import { decodePublicKey, signatureFindings } from "./signatures";

const APP_DIRECTORY = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const TAURI_DIRECTORY = join(APP_DIRECTORY, "src-tauri");
const REPOSITORY = resolve(APP_DIRECTORY, "../..");
const RELEASE_CONFIG = join(TAURI_DIRECTORY, "tauri.release.conf.json");
/** Where `just release-bundle` copies the payload; `bundle.resources` reads it. */
const BUNDLED_PAYLOAD = join(TAURI_DIRECTORY, "resources", "jet-core.tar.gz");
/** The Cargo `[[bin]]` name, which the bundles install under usr/bin. */
const MAIN_BINARY = "jet-tauri";

class UsageError extends Error {}

type TauriConfig = {
  productName?: string;
  version?: string;
  build?: { frontendDist?: string };
  plugins?: { updater?: { pubkey?: string; endpoints?: string[] } };
};

function readJson<T>(path: string): T {
  return JSON.parse(readFileSync(path, "utf8")) as T;
}

function tauriConfig(): TauriConfig {
  return readJson<TauriConfig>(join(TAURI_DIRECTORY, "tauri.conf.json"));
}

/** The release configuration's updater public key and endpoints. */
function releaseUpdater(): { pubkey: string; endpoints: string[] } {
  const updater = readJson<TauriConfig>(RELEASE_CONFIG).plugins?.updater;
  if (updater?.pubkey === undefined || !updater.endpoints?.length) {
    throw new UsageError("tauri.release.conf.json has no plugins.updater pubkey and endpoints");
  }
  return { pubkey: updater.pubkey, endpoints: updater.endpoints };
}

function appVersion(): string {
  const version = tauriConfig().version;
  if (version === undefined) throw new UsageError("src-tauri/tauri.conf.json has no version");
  return version;
}

function productName(): string {
  const name = tauriConfig().productName;
  if (name === undefined) throw new UsageError("src-tauri/tauri.conf.json has no productName");
  return name;
}

function hostArch(): LinuxArch {
  const arch = process.platform === "linux" ? linuxArch(process.arch) : undefined;
  if (arch === undefined) throw new UsageError(`Linux bundles are built on Linux x86_64 or aarch64, not ${process.platform} ${process.arch}`);
  return arch;
}

function report(findings: string[], success: string): number {
  if (findings.length === 0) {
    console.log(success);
    return 0;
  }
  for (const finding of findings) console.error(`FAIL ${finding}`);
  return 1;
}

// ---------------------------------------------------------------------------

function versionCheck(): number {
  const overlay = existsSync(RELEASE_CONFIG) ? readJson<Record<string, unknown>>(RELEASE_CONFIG) : {};
  const findings = versionFindings({
    workspace: tomlString(readFileSync(join(REPOSITORY, "packages/Cargo.toml"), "utf8"), "workspace.package", "version"),
    tauriConfig: tauriConfig().version,
    cargoToml: tomlString(readFileSync(join(TAURI_DIRECTORY, "Cargo.toml"), "utf8"), "package", "version"),
    cargoLock: cargoLockVersion(readFileSync(join(TAURI_DIRECTORY, "Cargo.lock"), "utf8"), "jet-tauri"),
    packageJson: readJson<{ version?: string }>(join(APP_DIRECTORY, "package.json")).version,
    overlayHasVersion: "version" in overlay,
  });
  return report(findings, `App version ${appVersion()} matches the core workspace version.`);
}

function printBundleMode(): number {
  const decision = bundleMode(process.env.JET_RELEASE_SIGN);
  if ("finding" in decision) return report([decision.finding], "");
  console.error(
    decision.mode === "release"
      ? "JET_RELEASE_SIGN is on: a release build with plugins.updater, signed afterwards by `just release-sign`."
      : "JET_RELEASE_SIGN is off: an unsigned build without plugins.updater.",
  );
  console.log(decision.mode);
  return 0;
}

function checkPayload(path: string): number {
  const findings = payloadFindings(basename(path), readFileSync(path), appVersion(), targetTriple(hostArch()));
  return report(findings, `Core payload ${basename(path)} matches this app release.`);
}

/**
 * Cargo's target directory, honouring `CARGO_TARGET_DIR` and Cargo config as
 * `tauri build` sees them: Cargo reads config from the working directory up,
 * and `tauri build` runs Cargo in src-tauri.
 */
function targetDirectory(): string {
  const result = spawnSync(
    "cargo",
    ["metadata", "--format-version", "1", "--no-deps", "--offline", "--manifest-path", join(TAURI_DIRECTORY, "Cargo.toml")],
    { cwd: TAURI_DIRECTORY, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], maxBuffer: 64 * 1024 * 1024 },
  );
  if (result.status !== 0) throw new UsageError("cargo metadata failed; cannot locate the target directory");
  const directory = (JSON.parse(result.stdout) as { target_directory?: string }).target_directory;
  if (!directory) throw new UsageError("cargo metadata named no target directory");
  return directory;
}

function bundleDirectory(): string {
  return join(targetDirectory(), "release", "bundle");
}

/** A path as given and, when a symlink leads there, as the real path. */
function spellings(path: string): string[] {
  const real = existsSync(path) ? realpathSync(path) : path;
  return real === path ? [path] : [path, real];
}

/** The Cargo home, whose registry sources end up in panic locations. */
function cargoHome(): string {
  return resolve(process.env.CARGO_HOME || join(homedir(), ".cargo"));
}

function rustflags(): number {
  // Most specific last: the target directory usually sits in the checkout.
  const remaps = [
    ...spellings(REPOSITORY).map((from) => ({ from, to: "/jet" })),
    ...spellings(cargoHome()).map((from) => ({ from, to: "/cargo" })),
    ...spellings(targetDirectory()).map((from) => ({ from, to: "/target" })),
  ];
  const caller = { encoded: process.env.CARGO_ENCODED_RUSTFLAGS, plain: process.env.RUSTFLAGS };
  process.stdout.write(releaseRustflags(caller, remaps));
  return 0;
}

/** Paths that name the build machine: the checkout, Cargo's directories, the builder's home. */
function buildPaths(target: string): string[] {
  return [REPOSITORY, cargoHome(), target, homedir()].flatMap(spellings);
}

function cleanBundles(): number {
  const bundles = bundleDirectory();
  for (const kind of ["deb", "rpm", "appimage"]) rmSync(join(bundles, kind), { recursive: true, force: true });
  console.log(`Removed earlier deb, rpm and AppImage output under ${bundles}.`);
  return 0;
}

// ---------------------------------------------------------------------------

type TreeEntry = Pick<ArchiveEntry, "path" | "kind">;

/** Lists a directory without following symlinks. */
function walk(root: string): TreeEntry[] {
  const entries: TreeEntry[] = [];
  const visit = (directory: string): void => {
    for (const name of readdirSync(directory)) {
      const path = join(directory, name);
      const stat = lstatSync(path);
      const kind: EntryKind = stat.isSymbolicLink()
        ? "symlink"
        : stat.isDirectory()
          ? "directory"
          : stat.isFile()
            ? "file"
            : "other";
      entries.push({ path: relative(root, path), kind });
      if (kind === "directory") visit(path);
    }
  };
  visit(root);
  return entries;
}

/**
 * Extracts an AppImage with its own runtime (`--appimage-extract` needs no
 * FUSE and runs none of the app) into a fresh temporary directory.
 */
function extractAppImage(appImage: string, into: string): string {
  // Some runtimes let APPIMAGE_EXTRACT_AND_RUN win over the argument and
  // would start the app, so the child never inherits it.
  const { APPIMAGE_EXTRACT_AND_RUN: _ignored, ...env } = process.env;
  const result = spawnSync(appImage, ["--appimage-extract"], {
    cwd: into,
    stdio: ["ignore", "ignore", "pipe"],
    encoding: "utf8",
    env,
  });
  const root = join(into, "squashfs-root");
  if (result.status !== 0 || !existsSync(root)) {
    throw new Error(`--appimage-extract failed: ${result.error?.message ?? result.stderr.trim()}`);
  }
  return root;
}

type Contents = { entries: TreeEntry[]; payload: Buffer | undefined; binary: Buffer | undefined };

function bundleContents(bundle: ExpectedBundle, path: string, product: string, scratch: string): Contents {
  const wanted = { payload: payloadEntryPath(product), binary: `usr/bin/${MAIN_BINARY}` };
  if (bundle.kind === "appimage") {
    const root = extractAppImage(path, scratch);
    const entries = walk(root);
    const read = (entryPath: string): Buffer | undefined =>
      entries.some((entry) => entry.path === entryPath && entry.kind === "file")
        ? readFileSync(join(root, entryPath))
        : undefined;
    return { entries, payload: read(wanted.payload), binary: read(wanted.binary) };
  }
  const entries = bundle.kind === "deb" ? debEntries(readFileSync(path)) : rpmEntries(readFileSync(path));
  const read = (entryPath: string): Buffer | undefined =>
    entries.find((entry) => entry.path === entryPath && entry.kind === "file")?.data;
  return { entries, payload: read(wanted.payload), binary: read(wanted.binary) };
}

function webviewAssets(): { path: string; data: Buffer }[] {
  const dist = tauriConfig().build?.frontendDist;
  if (dist === undefined) throw new UsageError("tauri.conf.json has no build.frontendDist");
  const root = resolve(TAURI_DIRECTORY, dist);
  if (!existsSync(root)) throw new UsageError(`webview assets ${root} do not exist; build first`);
  return walk(root)
    .filter((entry) => entry.kind === "file")
    .map((entry) => ({ path: entry.path, data: readFileSync(join(root, entry.path)) }));
}

type BuiltBundles = { directory: string; bundles: ExpectedBundle[]; paths: string[]; findings: string[] };

/** This machine's deb, rpm and AppImage under `target`, each alone in its directory. */
function builtBundles(target: string, version: string): BuiltBundles {
  const directory = join(target, "release", "bundle");
  const bundles = expectedBundles(productName(), version, hostArch());
  const paths = bundles.map((bundle) => join(directory, bundle.directory, bundle.fileName));
  const findings: string[] = [];
  for (const [index, bundle] of bundles.entries()) {
    const kindDirectory = join(directory, bundle.directory);
    const present = existsSync(kindDirectory)
      ? readdirSync(kindDirectory, { withFileTypes: true })
          .filter((entry) => entry.isFile() && !entry.name.endsWith(".sig"))
          .map((entry) => entry.name)
      : [];
    const unexpected = present.filter((name) => name !== bundle.fileName);
    if (!existsSync(paths[index])) findings.push(`${bundle.kind}: ${bundle.fileName} is missing (found ${present.join(", ") || "nothing"})`);
    else if (unexpected.length > 0) findings.push(`${bundle.kind}: unexpected ${unexpected.join(", ")} next to ${bundle.fileName}`);
  }
  return { directory, bundles, paths, findings };
}

/**
 * This machine's deb, rpm and AppImage in a directory of their own, as the CI
 * signing job downloads them from the build job: nothing else may be there
 * apart from their `.sig` files.
 */
function bundlesIn(directory: string, version: string): BuiltBundles {
  const bundles = expectedBundles(productName(), version, hostArch());
  const entries: DirectoryEntry[] = readdirSync(directory, { withFileTypes: true }).map((entry) => ({
    name: entry.name,
    kind: entry.isSymbolicLink() ? "symlink" : entry.isDirectory() ? "directory" : entry.isFile() ? "file" : "other",
  }));
  const paths = bundles.map((bundle) => join(directory, bundle.fileName));
  return { directory, bundles, paths, findings: bundleDirectoryFindings(entries, bundles) };
}

function printBundles(directory: string | undefined): number {
  const version = appVersion();
  const { paths, findings } = directory === undefined ? builtBundles(targetDirectory(), version) : bundlesIn(directory, version);
  if (findings.length > 0) return report(findings, "");
  for (const path of paths) console.log(path);
  return 0;
}

/**
 * Verifies the updater signature of each bundle in `directory` against
 * `plugins.updater.pubkey`, the file name and the app version. It never opens
 * or runs a bundle, so the signing job needs no build to run it.
 */
function checkSignatures(directory: string): number {
  const version = appVersion();
  const { bundles, paths, findings } = bundlesIn(directory, version);
  if (findings.length > 0) return report(findings, "");
  const publicKey = decodePublicKey(releaseUpdater().pubkey);
  for (const [index, bundle] of bundles.entries()) {
    const signature = `${paths[index]}.sig`;
    if (!existsSync(signature)) {
      findings.push(`${bundle.kind}: ${bundle.fileName}.sig is missing`);
      continue;
    }
    for (const finding of signatureFindings(readFileSync(paths[index]), readFileSync(signature, "utf8"), publicKey, bundle.fileName, version)) {
      findings.push(`${bundle.kind}: ${bundle.fileName}.sig: ${finding}`);
    }
  }
  return report(findings, `Updater signatures of ${bundles.map((bundle) => bundle.fileName).join(", ")} verify for ${version}.`);
}

function verify(payloadPath: string, options: Pick<VerifyOptions, "release" | "requireSignatures">): number {
  const product = productName();
  const version = appVersion();
  const arch = hostArch();
  const payload = readFileSync(payloadPath);
  const target = targetDirectory();
  const machinePaths = buildPaths(target);
  const { directory, bundles, paths, findings } = builtBundles(target, version);
  const rows: string[] = [];
  if (findings.length > 0) return report(findings, "");

  const updater = releaseUpdater();
  const signed = paths.map((path) => existsSync(`${path}.sig`));
  const state = signatureState(signed);
  // A signed bundle is a release build even when `--release` is not given.
  const release = options.release || state === "all";

  const scratch = mkdtempSync(join(tmpdir(), "jet-release-verify-"));
  try {
    for (const [index, bundle] of bundles.entries()) {
      let contents: Contents;
      try {
        contents = bundleContents(bundle, paths[index], product, scratch);
      } catch (error) {
        findings.push(`${bundle.kind}: cannot read ${bundle.fileName}: ${(error as Error).message}`);
        continue;
      }
      const { entries, payload: bundled, binary } = contents;
      const bundleFindings = bundleEntryFindings(entries, product, MAIN_BINARY);
      if (binary !== undefined) {
        bundleFindings.push(
          ...bundleTypeFindings(bundle.kind, binary, MAIN_BINARY),
          ...binaryBuildPathFindings(binary, MAIN_BINARY, machinePaths),
        );
        // A partial signature set is its own finding below.
        if (state !== "partial") bundleFindings.push(...updaterEndpointFindings(binary, MAIN_BINARY, updater.endpoints, release));
      }
      if (bundled !== undefined && !bundled.equals(payload)) {
        bundleFindings.push(`${payloadEntryPath(product)} differs from ${payloadPath} (sha256 ${sha256(bundled)} vs ${sha256(payload)})`);
      }
      for (const finding of bundleFindings) findings.push(`${bundle.kind}: ${finding}`);
      rmSync(join(scratch, "squashfs-root"), { recursive: true, force: true });
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }

  for (const finding of webviewAssetFindings(webviewAssets(), machinePaths)) findings.push(`webview: ${finding}`);

  if (state === "partial") {
    findings.push(`only some bundles are signed: ${bundles.filter((_, i) => !signed[i]).map((b) => b.fileName).join(", ")} lack a .sig`);
  } else if (state === "none" && options.requireSignatures) {
    findings.push("no updater signatures, but signing was requested; sign the bundles with `just release-sign`");
  } else if (state === "all") {
    const publicKey = decodePublicKey(updater.pubkey);
    for (const [index, bundle] of bundles.entries()) {
      const signature = readFileSync(`${paths[index]}.sig`, "utf8");
      for (const finding of signatureFindings(readFileSync(paths[index]), signature, publicKey, bundle.fileName, version)) {
        findings.push(`${bundle.kind}: ${bundle.fileName}.sig: ${finding}`);
      }
    }
  }

  for (const [index, bundle] of bundles.entries()) {
    const data = readFileSync(paths[index]);
    rows.push(`${bundle.fileName}  ${data.length} bytes (${formatSize(data.length)})  sha256 ${sha256(data)}`);
    if (signed[index]) rows.push(`${bundle.fileName}.sig  ${lstatSync(`${paths[index]}.sig`).size} bytes`);
  }
  rows.push(`payload ${basename(payloadPath)}  ${payload.length} bytes (${formatSize(payload.length)})  sha256 ${sha256(payload)}`);
  console.log(`Bundles in ${directory}:`);
  for (const row of rows) console.log(`  ${row}`);
  const signatures =
    state === "all"
      ? "updater signatures verified"
      : release
        ? "a release build with the updater endpoint, not yet signed (`just release-sign`)"
        : "unsigned, without updater endpoints";
  return report(findings, `Release bundles ${version} (${arch}) passed: payload byte-identical in deb, rpm and AppImage; ${signatures}.`);
}

// ---------------------------------------------------------------------------

function main(args: string[]): number {
  const [command, ...rest] = args;
  switch (command) {
    case "version-check":
      return versionCheck();
    case "app-version":
      console.log(appVersion());
      return 0;
    case "check-payload": {
      if (rest.length !== 1) throw new UsageError("check-payload takes one archive path");
      return checkPayload(realpathSync(rest[0]));
    }
    case "clean-bundles":
      return cleanBundles();
    case "bundle-mode":
      return printBundleMode();
    case "rustflags":
      return rustflags();
    case "bundles": {
      if (rest.length === 0) return printBundles(undefined);
      if (rest.length !== 2 || rest[0] !== "--in") throw new UsageError("bundles takes nothing, or --in <directory>");
      return printBundles(realpathSync(rest[1]));
    }
    case "check-signatures": {
      if (rest.length !== 1) throw new UsageError("check-signatures takes one directory");
      return checkSignatures(realpathSync(rest[0]));
    }
    case "verify": {
      const options = verifyOptions(rest);
      if ("error" in options) throw new UsageError(options.error);
      return verify(realpathSync(options.payload ?? BUNDLED_PAYLOAD), options);
    }
    default:
      throw new UsageError(
        "usage: main.ts version-check | app-version | bundle-mode | check-payload <archive> | clean-bundles | rustflags | bundles [--in <directory>] | verify [--payload <archive>] [--release] [--require-signatures] | check-signatures <directory>",
      );
  }
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(error instanceof UsageError ? error.message : `release check could not run: ${(error as Error).message}`);
  process.exitCode = 2;
}
