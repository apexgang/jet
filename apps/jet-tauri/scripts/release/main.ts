/**
 * Release checks for the Linux desktop bundles, run through `just`:
 *
 *   bun scripts/release/main.ts version-check
 *   bun scripts/release/main.ts signing
 *   bun scripts/release/main.ts check-payload <jet-core-<v>-<target>.tar.gz>
 *   bun scripts/release/main.ts clean-bundles
 *   bun scripts/release/main.ts rustflags
 *   bun scripts/release/main.ts verify [--payload <archive>] [--require-signatures]
 *
 * `version-check` fails unless tauri.conf.json, src-tauri/Cargo.toml (and its
 * Cargo.lock entry) and package.json carry the core workspace version
 * (ADR-0053). `signing` prints `signed` or `unsigned` from `JET_RELEASE_SIGN`
 * and whether `TAURI_SIGNING_PRIVATE_KEY` is set, and fails when signing is
 * required without the key. `check-payload` validates the core payload
 * archive before a long build. `clean-bundles` removes earlier deb, rpm and
 * AppImage output so no stale bundle or `.sig` survives into a new build.
 * `rustflags` prints the `CARGO_ENCODED_RUSTFLAGS` that remap build paths out
 * of the app binary. `verify` checks the built deb, rpm and AppImage:
 * expected names, the payload byte-identical at `usr/lib/Jet/jet-core.tar.gz`,
 * the bundle type patched into each app binary, no build paths in it and the
 * updater endpoint only when signed, no source maps or fixtures, clean
 * webview assets, and updater signatures that verify against
 * `plugins.updater.pubkey` (all three or none; `--require-signatures` makes
 * none a failure). Without `--payload` it compares against the copy in
 * `src-tauri/resources/`. It prints each bundle's size and SHA-256. Relative
 * paths resolve against the working directory.
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
  cargoLockVersion,
  expectedBundles,
  formatSize,
  linuxArch,
  payloadEntryPath,
  payloadFindings,
  releaseRustflags,
  sha256,
  signatureState,
  signingDecision,
  targetTriple,
  tomlString,
  bundleEntryFindings,
  bundleTypeFindings,
  updaterEndpointFindings,
  verifyOptions,
  versionFindings,
  webviewAssetFindings,
  type ExpectedBundle,
  type LinuxArch,
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

function signing(): number {
  const decision = signingDecision(process.env.JET_RELEASE_SIGN, Boolean(process.env.TAURI_SIGNING_PRIVATE_KEY));
  if ("finding" in decision) return report([decision.finding], "");
  if (decision.note !== undefined) console.error(decision.note);
  console.log(decision.signing);
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

function verify(payloadPath: string, requireSignatures: boolean): number {
  const product = productName();
  const version = appVersion();
  const arch = hostArch();
  const payload = readFileSync(payloadPath);
  const target = targetDirectory();
  const directory = join(target, "release", "bundle");
  const machinePaths = buildPaths(target);
  const bundles = expectedBundles(product, version, arch);
  const findings: string[] = [];
  const rows: string[] = [];

  const paths = bundles.map((bundle) => join(directory, bundle.directory, bundle.fileName));
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
  if (findings.length > 0) return report(findings, "");

  const updater = readJson<TauriConfig>(RELEASE_CONFIG).plugins?.updater;
  if (updater?.pubkey === undefined || !updater.endpoints?.length) {
    throw new UsageError("tauri.release.conf.json has no plugins.updater pubkey and endpoints");
  }
  const signed = paths.map((path) => existsSync(`${path}.sig`));
  const state = signatureState(signed);

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
        if (state !== "partial") bundleFindings.push(...updaterEndpointFindings(binary, MAIN_BINARY, updater.endpoints, state === "all"));
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
  } else if (state === "none" && requireSignatures) {
    findings.push("no updater signatures, but signing was requested");
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
  const signatures = state === "all" ? "updater signatures verified" : "unsigned, without updater endpoints";
  return report(findings, `Release bundles ${version} (${arch}) passed: payload byte-identical in deb, rpm and AppImage; ${signatures}.`);
}

// ---------------------------------------------------------------------------

function main(args: string[]): number {
  const [command, ...rest] = args;
  switch (command) {
    case "version-check":
      return versionCheck();
    case "check-payload": {
      if (rest.length !== 1) throw new UsageError("check-payload takes one archive path");
      return checkPayload(realpathSync(rest[0]));
    }
    case "clean-bundles":
      return cleanBundles();
    case "signing":
      return signing();
    case "rustflags":
      return rustflags();
    case "verify": {
      const options = verifyOptions(rest);
      if ("error" in options) throw new UsageError(options.error);
      return verify(realpathSync(options.payload ?? BUNDLED_PAYLOAD), options.requireSignatures);
    }
    default:
      throw new UsageError(
        "usage: main.ts version-check | signing | check-payload <archive> | clean-bundles | rustflags | verify [--payload <archive>] [--require-signatures]",
      );
  }
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  console.error(error instanceof UsageError ? error.message : `release check could not run: ${(error as Error).message}`);
  process.exitCode = 2;
}
