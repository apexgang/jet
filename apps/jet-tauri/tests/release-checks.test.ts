import { createHash, generateKeyPairSync, randomBytes, sign, type KeyObject } from "node:crypto";
import { gzipSync } from "node:zlib";
import { describe, expect, it } from "vitest";

import releaseConfig from "../src-tauri/tauri.release.conf.json";
import {
  binaryBuildPathFindings,
  bundleDirectoryFindings,
  bundleEntryFindings,
  bundleMode,
  bundleTypeFindings,
  cargoLockVersion,
  expectedBundles,
  linuxArch,
  payloadFindings,
  releaseRustflags,
  sha256,
  signatureState,
  tomlString,
  updaterEndpointFindings,
  verifyOptions,
  versionFindings,
  webviewAssetFindings,
  type VersionSources,
} from "../scripts/release/checks";
import {
  decodePublicKey,
  hexKeyId,
  signatureFindings,
  trustedCommentFields,
} from "../scripts/release/signatures";

describe("version stamps (ADR-0053)", () => {
  const workspaceToml = [
    "[workspace]",
    'members = ["jet-daemon"]',
    "",
    "[workspace.package]",
    'version = "0.2.0" # the release version',
    'edition = "2024"',
    "",
    "[workspace.dependencies]",
    'version = "9.9.9"',
  ].join("\n");

  it("reads one key from one TOML section", () => {
    expect(tomlString(workspaceToml, "workspace.package", "version")).toBe("0.2.0");
    expect(tomlString(workspaceToml, "workspace.package", "license")).toBeUndefined();
    expect(tomlString(workspaceToml, "package", "version")).toBeUndefined();
  });

  it("reads a package's version from Cargo.lock", () => {
    const lock = [
      "version = 4",
      "",
      "[[package]]",
      'name = "jet-client"',
      'version = "0.2.0"',
      "",
      "[[package]]",
      'name = "jet-tauri"',
      'version = "0.1.0"',
      "dependencies = [",
      ' "jet-client",',
      "]",
    ].join("\n");
    expect(cargoLockVersion(lock, "jet-tauri")).toBe("0.1.0");
    expect(cargoLockVersion(lock, "missing")).toBeUndefined();
  });

  const agreeing: VersionSources = {
    workspace: "0.2.0",
    tauriConfig: "0.2.0",
    cargoToml: "0.2.0",
    cargoLock: "0.2.0",
    packageJson: "0.2.0",
    overlayHasVersion: false,
  };

  it("passes when every stamp equals the workspace version", () => {
    expect(versionFindings(agreeing)).toEqual([]);
  });

  it("names each stamp that differs or is missing", () => {
    const findings = versionFindings({ ...agreeing, tauriConfig: "0.1.0", packageJson: undefined });
    expect(findings).toEqual([
      "src-tauri/tauri.conf.json version is 0.1.0, not the workspace version 0.2.0",
      "package.json version is missing, not the workspace version 0.2.0",
    ]);
  });

  it("refuses a release overlay that sets its own version", () => {
    expect(versionFindings({ ...agreeing, overlayHasVersion: true })).toHaveLength(1);
  });

  it("fails without a workspace version", () => {
    expect(versionFindings({ ...agreeing, workspace: undefined })).toHaveLength(1);
  });
});

describe("expected bundles", () => {
  it("uses tauri-bundler's names on both Linux architectures", () => {
    expect(expectedBundles("Jet", "0.2.0", "x86_64").map((bundle) => `${bundle.directory}/${bundle.fileName}`)).toEqual([
      "deb/Jet_0.2.0_amd64.deb",
      "rpm/Jet-0.2.0-1.x86_64.rpm",
      "appimage/Jet_0.2.0_amd64.AppImage",
    ]);
    expect(expectedBundles("Jet", "0.2.0", "aarch64").map((bundle) => bundle.fileName)).toEqual([
      "Jet_0.2.0_arm64.deb",
      "Jet-0.2.0-1.aarch64.rpm",
      "Jet_0.2.0_aarch64.AppImage",
    ]);
  });

  it("maps Node architectures and refuses others", () => {
    expect(linuxArch("x64")).toBe("x86_64");
    expect(linuxArch("arm64")).toBe("aarch64");
    expect(linuxArch("ia32")).toBeUndefined();
  });

  describe("in the directory the signing job receives", () => {
    const expected = expectedBundles("Jet", "0.2.0", "x86_64");
    const files = (...names: string[]) => names.map((name) => ({ name, kind: "file" as const }));
    const bundles = ["Jet_0.2.0_amd64.deb", "Jet-0.2.0-1.x86_64.rpm", "Jet_0.2.0_amd64.AppImage"];

    it("accepts the three bundles, before and after signing", () => {
      expect(bundleDirectoryFindings(files(...bundles), expected)).toEqual([]);
      expect(bundleDirectoryFindings(files(...bundles, ...bundles.map((name) => `${name}.sig`)), expected)).toEqual([]);
    });

    it("names a missing bundle and anything else in the directory", () => {
      // Only what `release-sign` signs may travel on into the release.
      expect(
        bundleDirectoryFindings(files(bundles[0], bundles[2], "Jet_0.2.0_arm64.deb", "latest.json", "notes.sig"), expected),
      ).toEqual([
        "rpm: Jet-0.2.0-1.x86_64.rpm is missing",
        "unexpected Jet_0.2.0_arm64.deb",
        "unexpected latest.json",
        "unexpected notes.sig",
      ]);
    });

    it("refuses a bundle or signature that is not a regular file", () => {
      const entries = [
        ...files(bundles[0], bundles[1]),
        { name: bundles[2], kind: "symlink" as const },
        { name: `${bundles[0]}.sig`, kind: "directory" as const },
      ];
      expect(bundleDirectoryFindings(entries, expected)).toEqual([
        "Jet_0.2.0_amd64.AppImage is a symlink, not a regular file",
        "Jet_0.2.0_amd64.deb.sig is a directory, not a regular file",
      ]);
    });
  });
});

// ---------------------------------------------------------------------------

function tarFile(path: string, data: Buffer): Buffer {
  const header = Buffer.alloc(512);
  header.write(path, 0, 100, "utf8");
  header.write(`${data.length.toString(8).padStart(11, "0")}\0`, 124, "latin1");
  header.write("0", 156, "latin1");
  header.write("ustar\u000000", 257, "latin1");
  return Buffer.concat([header, data, Buffer.alloc((512 - (data.length % 512)) % 512)]);
}

function payloadArchive(files: Record<string, Buffer>): Buffer {
  const blocks = Object.entries(files).map(([path, data]) => tarFile(path, data));
  return gzipSync(Buffer.concat([...blocks, Buffer.alloc(1024)]));
}

describe("payloadFindings", () => {
  const version = "0.2.0";
  const target = "x86_64-unknown-linux-gnu";
  const stem = `jet-core-${version}-${target}`;
  const executables = Object.fromEntries(
    ["jet-craft-claude", "jet-craft-codex", "jetd", "jetfueld"].map((name) => [name, Buffer.from(`${name} ELF`)]),
  );
  const manifest = (overrides: Record<string, unknown> = {}): Buffer =>
    Buffer.from(
      JSON.stringify({
        executables: Object.fromEntries(Object.entries(executables).map(([name, data]) => [name, sha256(data)])),
        target,
        version,
        ...overrides,
      }),
    );
  const files = (manifestBytes: Buffer): Record<string, Buffer> => ({
    ...Object.fromEntries(Object.entries(executables).map(([name, data]) => [`${stem}/${name}`, data])),
    [`${stem}/manifest.json`]: manifestBytes,
  });

  it("accepts the release archive for this version and target", () => {
    expect(payloadFindings(`${stem}.tar.gz`, payloadArchive(files(manifest())), version, target)).toEqual([]);
  });

  it("refuses another version, another target, or a renamed file", () => {
    const archive = payloadArchive(files(manifest({ version: "0.1.0", target: "aarch64-unknown-linux-gnu" })));
    expect(payloadFindings("jet-core.tar.gz", archive, version, target)).toEqual([
      `payload file is jet-core.tar.gz, expected ${stem}.tar.gz`,
      "payload manifest version is 0.1.0, not 0.2.0",
      "payload manifest target is aarch64-unknown-linux-gnu, not x86_64-unknown-linux-gnu",
    ]);
  });

  it("refuses an executable that does not match its manifest digest", () => {
    const tampered = { ...files(manifest()), [`${stem}/jetd`]: Buffer.from("patched") };
    expect(payloadFindings(`${stem}.tar.gz`, payloadArchive(tampered), version, target)).toEqual([
      "payload jetd does not match its manifest digest",
    ]);
  });

  it("refuses loose files, extra top-level entries, and non-archives", () => {
    const loose = payloadArchive({ jetd: Buffer.from("x") });
    expect(payloadFindings(`${stem}.tar.gz`, loose, version, target)[0]).toMatch(/exactly one top-level directory/);
    const extra = payloadArchive({ ...files(manifest()), "other/file": Buffer.from("x") });
    expect(payloadFindings(`${stem}.tar.gz`, extra, version, target)[0]).toMatch(/exactly one top-level directory/);
    expect(payloadFindings(`${stem}.tar.gz`, Buffer.from("text"), version, target)[0]).toMatch(/not a readable/);
  });

  it("refuses an oversized manifest", () => {
    const archive = payloadArchive(files(Buffer.alloc(64 * 1024 + 1, 0x20)));
    expect(payloadFindings(`${stem}.tar.gz`, archive, version, target)).toEqual(["payload manifest.json exceeds 64 KiB"]);
  });
});

describe("bundle and webview content", () => {
  const complete = [
    { path: "usr/bin/jet-tauri", kind: "file" as const },
    { path: "usr/lib/Jet/jet-core.tar.gz", kind: "file" as const },
    { path: "usr/share/applications/Jet.desktop", kind: "file" as const },
  ];

  it("accepts a bundle with the binary and the payload", () => {
    expect(bundleEntryFindings(complete, "Jet", "jet-tauri")).toEqual([]);
  });

  it("names a missing payload, source maps and fixtures", () => {
    const findings = bundleEntryFindings(
      [
        complete[0],
        { path: "usr/lib/Jet/jet-core.tar.gz", kind: "directory" },
        { path: "usr/lib/Jet/app.js.map", kind: "file" },
        { path: "usr/lib/Jet/fixtures/desktop/presentation-v1.json", kind: "file" },
        { path: "usr/share/X11/locale/compose.map", kind: "file" },
      ],
      "Jet",
      "jet-tauri",
    );
    expect(findings).toEqual([
      "usr/lib/Jet/jet-core.tar.gz is missing",
      "usr/lib/Jet/app.js.map is a source map",
      "usr/lib/Jet/fixtures/desktop/presentation-v1.json is a development fixture",
    ]);
  });

  it("checks the webview assets for maps, fixture paths, home directories and build paths", () => {
    const findings = webviewAssetFindings(
      [
        { path: "index.html", data: Buffer.from("<script src=/_app/start.js></script>") },
        { path: "_app/start.js.map", data: Buffer.from("{}") },
        { path: "_app/chunk.js", data: Buffer.from("x()\n//# sourceMappingURL=chunk.js.map") },
        { path: "_app/fixture.js", data: Buffer.from('import "../../fixtures/desktop/presentation-v1.json"') },
        { path: "_app/leak.js", data: Buffer.from('a="/home/you/code"; b="/home/builder/jet/apps"') },
        { path: "_app/mac.js", data: Buffer.from('const root = "/Users/builder/src/jet/apps"') },
      ],
      ["/Users/builder/src/jet"],
    );
    expect(findings).toEqual([
      "_app/start.js.map is a source map",
      "_app/chunk.js links a source map",
      "_app/fixture.js names fixtures/desktop",
      "_app/leak.js contains the absolute path /home/builder",
      "_app/mac.js contains the build path /Users/builder/src/jet",
    ]);
  });

  it("allows the Setup panel's example project path", () => {
    const setup = Buffer.from('<input placeholder="/home/you/code/project">');
    expect(webviewAssetFindings([{ path: "_app/nodes/2.js", data: setup }])).toEqual([]);
  });

  it("requires the bundle type tauri-bundler patches into each binary", () => {
    const deb = Buffer.from("ELF...__TAURI_BUNDLE_TYPE_VAR_DEB...");
    expect(bundleTypeFindings("deb", deb, "jet-tauri")).toEqual([]);
    expect(bundleTypeFindings("appimage", Buffer.from("ELF...__TAURI_BUNDLE_TYPE_VAR_APP"), "jet-tauri")).toEqual([]);
    expect(bundleTypeFindings("rpm", deb, "jet-tauri")).toEqual([
      "usr/bin/jet-tauri does not record the rpm bundle type the updater needs",
    ]);
  });

  it("classifies which bundles are signed", () => {
    expect(signatureState([true, true, true])).toBe("all");
    expect(signatureState([false, false, false])).toBe("none");
    expect(signatureState([true, false, true])).toBe("partial");
  });

  it("finds build paths left in the app binary", () => {
    const binary = Buffer.from(
      "ELF\0/cargo/registry/src/serde-1.0/src/de.rs\0" +
        "/home/builder/.cargo/registry/src/index.crates.io-1/serde-1.0/src/ser.rs\0" +
        "/home/builder/src/jet/packages/jet-client/src/lib.rs\0/home/builder/src/jet/packages/jet-protocol/src/lib.rs\0" +
        "/home/builderx/other\0",
    );
    expect(
      binaryBuildPathFindings(binary, "jet-tauri", ["/home/builder/src/jet", "/home/builder/.cargo", "/home/builder", "/", ""]),
    ).toEqual([
      "usr/bin/jet-tauri contains the build path /home/builder/src/jet 2 times",
      "usr/bin/jet-tauri contains the build path /home/builder/.cargo 1 times",
      "usr/bin/jet-tauri contains the build path /home/builder 3 times",
    ]);
    const remapped = Buffer.from("ELF\0/jet/packages/jet-client/src/lib.rs\0/cargo/registry/src/x/serde-1.0/src/de.rs\0");
    expect(binaryBuildPathFindings(remapped, "jet-tauri", ["/home/builder/src/jet", "/home/builder"])).toEqual([]);
  });

  it("keeps the updater endpoint in release builds only", () => {
    const endpoints = releaseConfig.plugins.updater.endpoints;
    const withUpdater = Buffer.from(`ELF\0{"updater":{"endpoints":["${endpoints[0]}"]}}\0`);
    const withoutUpdater = Buffer.from("ELF\0https://github.com/apexgang/jet\0");
    expect(updaterEndpointFindings(withUpdater, "jet-tauri", endpoints, true)).toEqual([]);
    expect(updaterEndpointFindings(withoutUpdater, "jet-tauri", endpoints, false)).toEqual([]);
    expect(updaterEndpointFindings(withUpdater, "jet-tauri", endpoints, false)).toEqual([
      `usr/bin/jet-tauri is not a release build but carries the updater endpoint ${endpoints[0]} (verify a release build with --release)`,
    ]);
    expect(updaterEndpointFindings(withoutUpdater, "jet-tauri", endpoints, true)).toEqual([
      `usr/bin/jet-tauri is a release build but lacks the updater endpoint ${endpoints[0]}`,
    ]);
  });
});

describe("build inputs", () => {
  it.each([
    { request: undefined, mode: "unsigned" },
    { request: "", mode: "unsigned" },
    { request: "false", mode: "unsigned" },
    { request: "0", mode: "unsigned" },
    { request: "true", mode: "release" },
    { request: "1", mode: "release" },
  ] as const)("builds JET_RELEASE_SIGN=$request as $mode", ({ request, mode }) => {
    // The mode never depends on the updater key: no build signs.
    expect(bundleMode(request)).toEqual({ mode });
  });

  it("refuses a JET_RELEASE_SIGN it does not understand", () => {
    for (const request of ["yes", "TRUE", " true"]) {
      expect(bundleMode(request)).toEqual({ finding: `JET_RELEASE_SIGN is ${JSON.stringify(request)}; use true, false, 1 or 0` });
    }
  });

  const remaps = [
    { from: "/home/builder/src/jet", to: "/jet" },
    { from: "/home/builder/My Cargo", to: "/cargo" },
  ];
  const remapFlags = ["--remap-path-prefix=/home/builder/src/jet=/jet", "--remap-path-prefix=/home/builder/My Cargo=/cargo"];

  it("adds a remap per build path to the caller's rustc flags", () => {
    expect(releaseRustflags({ encoded: undefined, plain: undefined }, remaps).split("\x1f")).toEqual(remapFlags);
    expect(releaseRustflags({ encoded: undefined, plain: "  -C target-cpu=x86-64-v2 " }, remaps).split("\x1f")).toEqual([
      "-C",
      "target-cpu=x86-64-v2",
      ...remapFlags,
    ]);
  });

  it("prefers CARGO_ENCODED_RUSTFLAGS over RUSTFLAGS, as Cargo does", () => {
    const encoded = ["-C", "link-arg=-Wl,--as-needed"].join("\x1f");
    expect(releaseRustflags({ encoded, plain: "-C opt-level=0" }, remaps).split("\x1f")).toEqual([
      "-C",
      "link-arg=-Wl,--as-needed",
      ...remapFlags,
    ]);
    expect(releaseRustflags({ encoded: "", plain: "-C opt-level=0" }, remaps).split("\x1f")).toEqual(remapFlags);
  });

  it("parses verify's flags without a payload argument", () => {
    // `just release-verify --require-signatures` passes the flag alone.
    expect(verifyOptions(["--require-signatures"])).toEqual({ payload: undefined, release: true, requireSignatures: true });
    expect(verifyOptions([])).toEqual({ payload: undefined, release: false, requireSignatures: false });
    expect(verifyOptions(["--release"])).toEqual({ payload: undefined, release: true, requireSignatures: false });
    expect(verifyOptions(["--require-signatures", "--payload", "dist/core.tar.gz"])).toEqual({
      payload: "dist/core.tar.gz",
      release: true,
      requireSignatures: true,
    });
  });

  it("refuses a missing or repeated payload and unknown arguments", () => {
    const payloadError = { error: "--payload takes one archive path" };
    expect(verifyOptions(["--payload"])).toEqual(payloadError);
    expect(verifyOptions(["--payload", ""])).toEqual(payloadError);
    expect(verifyOptions(["--payload", "--require-signatures"])).toEqual(payloadError);
    expect(verifyOptions(["--payload", "a.tar.gz", "--payload", "b.tar.gz"])).toEqual(payloadError);
    expect(verifyOptions(["core.tar.gz"])).toEqual({ error: "unknown verify argument core.tar.gz" });
  });
});

// ---------------------------------------------------------------------------
// Updater signatures, produced here the way the Tauri CLI writes them.

type Signer = { privateKey: KeyObject; keyId: Buffer; pubkey: string };

function signer(): Signer {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const raw = Buffer.from(publicKey.export({ format: "jwk" }).x as string, "base64url");
  const keyId = randomBytes(8);
  const box = `untrusted comment: minisign public key: ${hexKeyId(keyId)}\n${Buffer.concat([Buffer.from("Ed"), keyId, raw]).toString("base64")}\n`;
  return { privateKey, keyId, pubkey: Buffer.from(box).toString("base64") };
}

function signatureFile(key: Signer, data: Buffer, comment: string, { prehashed = true, keyId = key.keyId } = {}): string {
  const message = prehashed ? createHash("blake2b512").update(data).digest() : data;
  const signature = sign(null, message, key.privateKey);
  const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), key.privateKey);
  const body = Buffer.concat([Buffer.from(prehashed ? "ED" : "Ed"), keyId, signature]).toString("base64");
  const box = `untrusted comment: signature from tauri secret key\n${body}\ntrusted comment: ${comment}\n${global.toString("base64")}\n`;
  return Buffer.from(box).toString("base64");
}

describe("updater signatures", () => {
  const data = Buffer.from("the deb bytes");
  const fileName = "Jet_0.2.0_amd64.deb";
  const comment = `timestamp:1790000000\tfile:${fileName}\tversion:0.2.0`;

  it("decodes the release public key with the expected key ID", () => {
    expect(hexKeyId(decodePublicKey(releaseConfig.plugins.updater.pubkey).keyId)).toBe("B530A38E9C125B31");
  });

  it("parses Tauri's trusted comment", () => {
    expect(Object.fromEntries(trustedCommentFields(comment))).toEqual({
      timestamp: "1790000000",
      file: fileName,
      version: "0.2.0",
    });
  });

  it("verifies prehashed and legacy signatures", () => {
    const key = signer();
    const publicKey = decodePublicKey(key.pubkey);
    expect(signatureFindings(data, signatureFile(key, data, comment), publicKey, fileName, "0.2.0")).toEqual([]);
    expect(signatureFindings(data, signatureFile(key, data, comment, { prehashed: false }), publicKey, fileName, "0.2.0")).toEqual([]);
  });

  it("refuses another key, changed content, a forged comment, or a wrong file or version", () => {
    const key = signer();
    const other = signer();
    const publicKey = decodePublicKey(key.pubkey);
    const good = signatureFile(key, data, comment);

    expect(signatureFindings(data, signatureFile(other, data, comment), publicKey, fileName, "0.2.0")[0]).toMatch(
      /signed with key .*, but plugins.updater.pubkey is/,
    );
    expect(signatureFindings(data, signatureFile(other, data, comment, { keyId: key.keyId }), publicKey, fileName, "0.2.0")).toEqual([
      "signature does not match the file content",
    ]);
    expect(signatureFindings(Buffer.from("other bytes"), good, publicKey, fileName, "0.2.0")).toEqual([
      "signature does not match the file content",
    ]);

    const forged = Buffer.from(good, "base64").toString().replace("version:0.2.0", "version:9.9.9");
    expect(signatureFindings(data, Buffer.from(forged).toString("base64"), publicKey, fileName, "9.9.9")).toEqual([
      "trusted comment is not covered by the global signature",
    ]);

    expect(signatureFindings(data, good, publicKey, "Jet_0.2.0_amd64.AppImage", "0.2.1")).toEqual([
      `trusted comment names file "${fileName}", not Jet_0.2.0_amd64.AppImage`,
      'trusted comment carries version "0.2.0", not 0.2.1',
    ]);
    const unversioned = signatureFile(key, data, `timestamp:1\tfile:${fileName}`);
    expect(signatureFindings(data, unversioned, publicKey, fileName, "0.2.0")).toEqual([
      "trusted comment carries version none, not 0.2.0",
    ]);
  });

  it("refuses text that is not a signature", () => {
    const publicKey = decodePublicKey(signer().pubkey);
    expect(signatureFindings(data, "not base64!", publicKey, fileName, "0.2.0")).toEqual(["signature file is not base64"]);
    expect(signatureFindings(data, Buffer.from("hello").toString("base64"), publicKey, fileName, "0.2.0")).toEqual([
      "signature file is not a minisign signature",
    ]);
  });
});
