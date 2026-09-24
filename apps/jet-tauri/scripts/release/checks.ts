/**
 * Pure release checks for the Linux desktop bundles: the version stamps
 * (ADR-0053), the core payload archive, the expected bundle names, what a
 * bundle, its app binary or the webview assets must not contain, and the
 * build's inputs (whether it signs, its rustc flags, the `verify` arguments).
 * `main.ts` does the file and process work; everything here takes bytes or
 * text and returns findings, so the tests need no bundle.
 */

import { createHash } from "node:crypto";

import { gunzip, readTar, type ArchiveEntry } from "./archives";

// ---------------------------------------------------------------------------
// Versions (ADR-0053: the GUIs and the core ship under one release version)

/**
 * A string value from one `[section]` of a TOML file. Enough for the
 * `version = "x"` lines checked here; not a TOML parser.
 */
export function tomlString(text: string, section: string, key: string): string | undefined {
  let inSection = false;
  for (const rawLine of text.split("\n")) {
    const line = rawLine.trim();
    if (line.startsWith("[")) {
      inSection = line === `[${section}]`;
      continue;
    }
    if (!inSection) continue;
    const match = /^([A-Za-z0-9_-]+)\s*=\s*"([^"]*)"\s*(#.*)?$/.exec(line);
    if (match && match[1] === key) return match[2];
  }
  return undefined;
}

/** The version of package `name` recorded in a Cargo.lock. */
export function cargoLockVersion(text: string, name: string): string | undefined {
  for (const block of text.split(/^\[\[package\]\]$/m)) {
    if (tomlString(`[p]\n${block}`, "p", "name") === name) {
      return tomlString(`[p]\n${block}`, "p", "version");
    }
  }
  return undefined;
}

export type VersionSources = {
  /** `packages/Cargo.toml` `[workspace.package].version`. */
  workspace: string | undefined;
  tauriConfig: string | undefined;
  cargoToml: string | undefined;
  cargoLock: string | undefined;
  packageJson: string | undefined;
  /** Whether the release overlay sets its own `version` (it must not). */
  overlayHasVersion: boolean;
};

export function versionFindings(sources: VersionSources): string[] {
  const { workspace } = sources;
  if (workspace === undefined) return ["packages/Cargo.toml has no [workspace.package] version"];
  const stamps: [string, string | undefined][] = [
    ["src-tauri/tauri.conf.json version", sources.tauriConfig],
    ["src-tauri/Cargo.toml [package] version", sources.cargoToml],
    ["src-tauri/Cargo.lock jet-tauri version", sources.cargoLock],
    ["package.json version", sources.packageJson],
  ];
  const findings = stamps
    .filter(([, value]) => value !== workspace)
    .map(([where, value]) => `${where} is ${value ?? "missing"}, not the workspace version ${workspace}`);
  if (sources.overlayHasVersion) {
    findings.push("src-tauri/tauri.release.conf.json sets version; the release must use tauri.conf.json's");
  }
  return findings;
}

// ---------------------------------------------------------------------------
// Targets and bundle names

export type LinuxArch = "x86_64" | "aarch64";

type ArchNames = { deb: string; rpm: string; appimage: string; target: string };

const ARCH_NAMES: Record<LinuxArch, ArchNames> = {
  x86_64: { deb: "amd64", rpm: "x86_64", appimage: "amd64", target: "x86_64-unknown-linux-gnu" },
  aarch64: { deb: "arm64", rpm: "aarch64", appimage: "aarch64", target: "aarch64-unknown-linux-gnu" },
};

/** Maps Node's `process.arch` to the bundle architecture, if supported. */
export function linuxArch(nodeArch: string): LinuxArch | undefined {
  return nodeArch === "x64" ? "x86_64" : nodeArch === "arm64" ? "aarch64" : undefined;
}

export function targetTriple(arch: LinuxArch): string {
  return ARCH_NAMES[arch].target;
}

export type BundleKind = "deb" | "rpm" | "appimage";

export type ExpectedBundle = {
  kind: BundleKind;
  /** Subdirectory of `<target dir>/release/bundle`. */
  directory: BundleKind;
  fileName: string;
};

/**
 * The file names tauri-bundler 2.9 gives each Linux bundle
 * (`linux/debian.rs`, `linux/rpm.rs`, `linux/appimage/linuxdeploy.rs`).
 * The release assets and the Homebrew cask depend on these names.
 */
export function expectedBundles(productName: string, version: string, arch: LinuxArch): ExpectedBundle[] {
  const names = ARCH_NAMES[arch];
  return [
    { kind: "deb", directory: "deb", fileName: `${productName}_${version}_${names.deb}.deb` },
    { kind: "rpm", directory: "rpm", fileName: `${productName}-${version}-1.${names.rpm}.rpm` },
    { kind: "appimage", directory: "appimage", fileName: `${productName}_${version}_${names.appimage}.AppImage` },
  ];
}

/** Where `bundle.resources` places the payload: `usr/lib/<productName>/`. */
export function payloadEntryPath(productName: string): string {
  return `usr/lib/${productName}/jet-core.tar.gz`;
}

// ---------------------------------------------------------------------------
// Core payload archive

/** `manifest.json` is small; the native side refuses anything larger too. */
export const MANIFEST_LIMIT = 64 * 1024;

export const PAYLOAD_EXECUTABLES = ["jet-craft-claude", "jet-craft-codex", "jetd", "jetfueld"] as const;

export function sha256(data: Buffer): string {
  return createHash("sha256").update(data).digest("hex");
}

/**
 * Problems with a core payload archive for this app release: the unmodified
 * `jet-core-<version>-<target>.tar.gz` from `just release-package`, holding
 * one top-level directory with the four executables and a manifest whose
 * version and target match and whose digests match the executables.
 */
export function payloadFindings(fileName: string, archive: Buffer, version: string, target: string): string[] {
  const stem = `jet-core-${version}-${target}`;
  const findings: string[] = [];
  if (fileName !== `${stem}.tar.gz`) findings.push(`payload file is ${fileName}, expected ${stem}.tar.gz`);

  let entries: ArchiveEntry[];
  try {
    entries = readTar(gunzip(archive, "payload"));
  } catch (error) {
    return [...findings, `payload is not a readable .tar.gz: ${(error as Error).message}`];
  }
  const roots = new Set(entries.map((entry) => entry.path.split("/")[0]).filter((root) => root !== ""));
  if (roots.size !== 1 || !roots.has(stem)) {
    return [...findings, `payload must hold exactly one top-level directory ${stem}, found ${[...roots].join(", ") || "none"}`];
  }
  const files = new Map(entries.filter((entry) => entry.kind === "file").map((entry) => [entry.path, entry.data]));
  const manifestBytes = files.get(`${stem}/manifest.json`);
  if (manifestBytes === undefined) return [...findings, "payload has no manifest.json"];
  if (manifestBytes.length > MANIFEST_LIMIT) return [...findings, "payload manifest.json exceeds 64 KiB"];

  let manifest: { version?: unknown; target?: unknown; executables?: unknown };
  try {
    manifest = JSON.parse(manifestBytes.toString("utf8"));
  } catch {
    return [...findings, "payload manifest.json is not JSON"];
  }
  if (manifest.version !== version) findings.push(`payload manifest version is ${String(manifest.version)}, not ${version}`);
  if (manifest.target !== target) findings.push(`payload manifest target is ${String(manifest.target)}, not ${target}`);
  const digests = (manifest.executables ?? {}) as Record<string, unknown>;
  for (const name of PAYLOAD_EXECUTABLES) {
    const data = files.get(`${stem}/${name}`);
    if (data === undefined) findings.push(`payload has no ${name}`);
    else if (digests[name] !== sha256(data)) findings.push(`payload ${name} does not match its manifest digest`);
  }
  return findings;
}

// ---------------------------------------------------------------------------
// Bundle and webview content

/** Source maps would ship readable sources and build paths. */
const SOURCE_MAP = /\.(?:[cm]?js|css)\.map$/i;

/** Development fixtures (the desktop presentation corpus) must not ship as files. */
const FIXTURE_PATH = "fixtures/desktop";

/** Problems with the file list of one installed bundle tree. */
export function bundleEntryFindings(
  entries: Pick<ArchiveEntry, "path" | "kind">[],
  productName: string,
  mainBinary: string,
): string[] {
  const findings: string[] = [];
  const payload = payloadEntryPath(productName);
  const byPath = new Map(entries.map((entry) => [entry.path, entry]));
  if (byPath.get(payload)?.kind !== "file") findings.push(`${payload} is missing`);
  if (byPath.get(`usr/bin/${mainBinary}`)?.kind !== "file") findings.push(`usr/bin/${mainBinary} is missing`);
  for (const entry of entries) {
    if (SOURCE_MAP.test(entry.path)) findings.push(`${entry.path} is a source map`);
    if (entry.path.includes(FIXTURE_PATH)) findings.push(`${entry.path} is a development fixture`);
  }
  return findings;
}

/**
 * tauri-bundler patches `__TAURI_BUNDLE_TYPE` in each bundle's copy of the
 * binary (`tauri-utils` `platform::bundle_type`); the updater picks its
 * install method from it and the app disables updates without it.
 */
const BUNDLE_TYPE_MARKERS: Record<BundleKind, string> = {
  deb: "__TAURI_BUNDLE_TYPE_VAR_DEB",
  rpm: "__TAURI_BUNDLE_TYPE_VAR_RPM",
  appimage: "__TAURI_BUNDLE_TYPE_VAR_APP",
};

/** Problems with the bundle type recorded in one bundle's app binary. */
export function bundleTypeFindings(kind: BundleKind, binary: Buffer, mainBinary: string): string[] {
  return binary.includes(BUNDLE_TYPE_MARKERS[kind])
    ? []
    : [`usr/bin/${mainBinary} does not record the ${kind} bundle type the updater needs`];
}

const HOME_PATH = /\/home\/[A-Za-z0-9._-]+/g;

/** Example paths the UI shows on purpose (the Setup project-path placeholder). */
const HOME_EXAMPLES = new Set(["/home/you"]);

/**
 * Problems with the built webview assets (`frontendDist`), which the Tauri
 * build embeds compressed into the app binary: no source maps or source-map
 * links, no fixture paths, and no absolute home-directory or build paths
 * (`buildPaths`: the checkout and the builder's home) that would leak the
 * build machine's layout or user name.
 */
export function webviewAssetFindings(files: { path: string; data: Buffer }[], buildPaths: string[] = []): string[] {
  const findings: string[] = [];
  for (const { path, data } of files) {
    if (path.endsWith(".map")) findings.push(`${path} is a source map`);
    const text = data.toString("latin1");
    if (text.includes("sourceMappingURL=")) findings.push(`${path} links a source map`);
    if (text.includes(FIXTURE_PATH)) findings.push(`${path} names ${FIXTURE_PATH}`);
    const home = [...text.matchAll(HOME_PATH)].find(([match]) => !HOME_EXAMPLES.has(match));
    if (home) findings.push(`${path} contains the absolute path ${home[0]}`);
    for (const buildPath of buildPaths) {
      if (buildPath.length > 1 && text.includes(buildPath)) findings.push(`${path} contains the build path ${buildPath}`);
    }
  }
  return findings;
}

function occurrences(data: Buffer, needle: string): number {
  let count = 0;
  for (let at = data.indexOf(needle); at !== -1; at = data.indexOf(needle, at + needle.length)) count += 1;
  return count;
}

/**
 * Problems with one bundle's app binary: an absolute build path (`buildPaths`:
 * the checkout, the Cargo home and target directory, the builder's home) left
 * in a panic location or an embedded string, which would name the build
 * machine's layout and user. `strip` removes symbols, not these.
 */
export function binaryBuildPathFindings(binary: Buffer, mainBinary: string, buildPaths: string[]): string[] {
  const findings: string[] = [];
  for (const buildPath of new Set(buildPaths)) {
    if (buildPath.length <= 1) continue;
    const count = occurrences(binary, buildPath.endsWith("/") ? buildPath : `${buildPath}/`);
    if (count > 0) findings.push(`usr/bin/${mainBinary} contains the build path ${buildPath} ${count} times`);
  }
  return findings;
}

/**
 * Problems with the updater configuration Tauri compiles into the app binary.
 * A signed build must carry every `plugins.updater` endpoint. An unsigned
 * build (a pull-request or end-to-end bundle) must carry none: the app
 * registers the updater whenever its configuration has one and checks the
 * endpoint after launch, so an unsigned bundle would contact github.com and
 * follow the live release feed.
 */
export function updaterEndpointFindings(
  binary: Buffer,
  mainBinary: string,
  endpoints: string[],
  signed: boolean,
): string[] {
  return endpoints.flatMap((endpoint) => {
    const present = binary.includes(endpoint);
    if (signed && !present) return [`usr/bin/${mainBinary} is signed but lacks the updater endpoint ${endpoint}`];
    if (!signed && present) return [`usr/bin/${mainBinary} is unsigned but carries the updater endpoint ${endpoint}`];
    return [];
  });
}

// ---------------------------------------------------------------------------
// Build inputs

export type Signing = "signed" | "unsigned";

export type SigningDecision = { signing: Signing; note?: string } | { finding: string };

/**
 * Whether `just release-bundle` writes updater signatures. `JET_RELEASE_SIGN`
 * (`request`) states it: `true` or `1` requires them, `false` or `0` never
 * signs. Unset or empty, a build signs exactly when it has the private key.
 * Required signing without a key is a finding before the long build, so a
 * release job whose secret is missing fails instead of shipping unsigned
 * bundles.
 */
export function signingDecision(request: string | undefined, keyPresent: boolean): SigningDecision {
  const unsigned = (note: string): SigningDecision => ({ signing: "unsigned", note });
  switch (request ?? "") {
    case "true":
    case "1":
      return keyPresent
        ? { signing: "signed" }
        : { finding: "JET_RELEASE_SIGN requires updater signatures, but TAURI_SIGNING_PRIVATE_KEY is unset or empty" };
    case "false":
    case "0":
      return unsigned("JET_RELEASE_SIGN is off: building without updater signatures.");
    case "":
      return keyPresent
        ? { signing: "signed" }
        : unsigned("TAURI_SIGNING_PRIVATE_KEY is unset: building without updater signatures.");
    default:
      return { finding: `JET_RELEASE_SIGN is ${JSON.stringify(request)}; use true, false, 1 or 0` };
  }
}

/** Cargo's separator in `CARGO_ENCODED_RUSTFLAGS`. */
const ENCODED_SEPARATOR = "\x1f";

/**
 * `CARGO_ENCODED_RUSTFLAGS` for a release build: the caller's rustc flags as
 * Cargo reads them (`CARGO_ENCODED_RUSTFLAGS` wins over space-separated
 * `RUSTFLAGS`; either replaces Cargo-config `build.rustflags`), plus a
 * `--remap-path-prefix` per build path. Panic locations otherwise keep the
 * absolute paths of registry crates and of path dependencies outside the
 * package (`packages/`), and Cargo's `trim-paths` is unstable. rustc applies
 * the last matching prefix, so `remaps` lists the most specific path last.
 * The encoded form keeps paths with spaces whole.
 */
export function releaseRustflags(
  caller: { encoded: string | undefined; plain: string | undefined },
  remaps: { from: string; to: string }[],
): string {
  const flags =
    caller.encoded !== undefined
      ? caller.encoded.split(ENCODED_SEPARATOR).filter((flag) => flag !== "")
      : (caller.plain ?? "").split(" ").map((flag) => flag.trim()).filter((flag) => flag !== "");
  for (const { from, to } of remaps) flags.push(`--remap-path-prefix=${from}=${to}`);
  return flags.join(ENCODED_SEPARATOR);
}

export type VerifyOptions = { payload: string | undefined; requireSignatures: boolean };

/** Parses `verify [--payload <archive>] [--require-signatures]`, in any order. */
export function verifyOptions(args: string[]): VerifyOptions | { error: string } {
  let payload: string | undefined;
  let requireSignatures = false;
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    const value = args[index + 1];
    if (arg === "--require-signatures") {
      requireSignatures = true;
    } else if (arg === "--payload") {
      if (payload !== undefined || !value || value.startsWith("--")) return { error: "--payload takes one archive path" };
      payload = value;
      index += 1;
    } else {
      return { error: `unknown verify argument ${arg}` };
    }
  }
  return { payload, requireSignatures };
}

// ---------------------------------------------------------------------------
// Signatures

export type SignatureState = "all" | "none" | "partial";

/** Whether every bundle, no bundle, or only some bundles carry a `.sig`. */
export function signatureState(present: boolean[]): SignatureState {
  if (present.every(Boolean)) return "all";
  return present.some(Boolean) ? "partial" : "none";
}

export function formatSize(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`;
}
