/**
 * Verifies the updater signatures `tauri build` writes next to each bundle
 * (`<bundle>.sig`) against the public key in the release configuration. The
 * Tauri CLI only warns when the signing key does not match that public key;
 * such a release would build and publish fine, then fail every update
 * install. The updater plugin performs the same checks at install time.
 *
 * Formats (minisign, as the Tauri CLI and updater plugin encode it):
 * - `plugins.updater.pubkey` is base64 of the public-key file text; its
 *   second line is base64 of `Ed` + key ID (8 bytes) + Ed25519 key (32).
 * - A `.sig` file is base64 of the signature file text: an untrusted comment,
 *   base64 of algorithm (`ED` prehashed with BLAKE2b-512, or legacy `Ed`) +
 *   key ID + signature (64), the trusted comment, and base64 of the global
 *   signature over signature || trusted comment.
 * - Tauri's trusted comment is `timestamp:<unix>\tfile:<name>\tversion:<v>`.
 */

import { createHash, createPublicKey, verify, type KeyObject } from "node:crypto";

export class SignatureError extends Error {}

const UNTRUSTED = "untrusted comment: ";
const TRUSTED = "trusted comment: ";

export type PublicKey = { keyId: Buffer; key: KeyObject };

export type Signature = {
  prehashed: boolean;
  keyId: Buffer;
  signature: Buffer;
  trustedComment: string;
  globalSignature: Buffer;
};

function strictBase64(text: string, what: string): Buffer {
  const trimmed = text.trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(trimmed) || trimmed.length % 4 !== 0) {
    throw new SignatureError(`${what} is not base64`);
  }
  return Buffer.from(trimmed, "base64");
}

function boxLines(base64Box: string, what: string): string[] {
  const text = strictBase64(base64Box, what).toString("utf8");
  return text.split("\n").map((line) => line.replace(/\r$/, ""));
}

/** Decodes `plugins.updater.pubkey`. */
export function decodePublicKey(base64Box: string): PublicKey {
  const lines = boxLines(base64Box, "updater public key");
  if (!lines[0]?.startsWith(UNTRUSTED) || lines[1] === undefined) {
    throw new SignatureError("updater public key is not a minisign public key");
  }
  const raw = strictBase64(lines[1], "updater public key body");
  if (raw.length !== 42 || raw.subarray(0, 2).toString("latin1") !== "Ed") {
    throw new SignatureError("updater public key is not an Ed25519 minisign key");
  }
  const key = createPublicKey({
    key: { kty: "OKP", crv: "Ed25519", x: raw.subarray(10).toString("base64url") },
    format: "jwk",
  });
  return { keyId: raw.subarray(2, 10), key };
}

/** Decodes the content of a Tauri `.sig` file. */
export function decodeSignature(base64Box: string): Signature {
  const lines = boxLines(base64Box, "signature file");
  const [untrusted, body, trusted, global] = lines;
  if (
    !untrusted?.startsWith(UNTRUSTED) ||
    body === undefined ||
    !trusted?.startsWith(TRUSTED) ||
    global === undefined
  ) {
    throw new SignatureError("signature file is not a minisign signature");
  }
  const raw = strictBase64(body, "signature");
  const algorithm = raw.subarray(0, 2).toString("latin1");
  if (raw.length !== 74 || (algorithm !== "ED" && algorithm !== "Ed")) {
    throw new SignatureError("signature is not an Ed25519 minisign signature");
  }
  const globalSignature = strictBase64(global, "global signature");
  if (globalSignature.length !== 64) throw new SignatureError("global signature has the wrong length");
  return {
    prehashed: algorithm === "ED",
    keyId: raw.subarray(2, 10),
    signature: raw.subarray(10),
    trustedComment: trusted.slice(TRUSTED.length),
    globalSignature,
  };
}

/** `key:value` fields of Tauri's tab-separated trusted comment. */
export function trustedCommentFields(comment: string): Map<string, string> {
  const fields = new Map<string, string>();
  for (const part of comment.split("\t")) {
    const colon = part.indexOf(":");
    if (colon > 0) fields.set(part.slice(0, colon), part.slice(colon + 1));
  }
  return fields;
}

/**
 * Problems with `signatureFile` as the updater signature of `data`, named
 * `fileName` and released as `version`; empty when it verifies.
 */
export function signatureFindings(
  data: Buffer,
  signatureFile: string,
  publicKey: PublicKey,
  fileName: string,
  version: string,
): string[] {
  let signature: Signature;
  try {
    signature = decodeSignature(signatureFile);
  } catch (error) {
    return [(error as Error).message];
  }
  if (!signature.keyId.equals(publicKey.keyId)) {
    return [
      `signed with key ${hexKeyId(signature.keyId)}, but plugins.updater.pubkey is ${hexKeyId(publicKey.keyId)}`,
    ];
  }
  const message = signature.prehashed ? createHash("blake2b512").update(data).digest() : data;
  if (!verify(null, message, publicKey.key, signature.signature)) {
    return ["signature does not match the file content"];
  }
  const signedComment = Buffer.concat([signature.signature, Buffer.from(signature.trustedComment, "utf8")]);
  if (!verify(null, signedComment, publicKey.key, signature.globalSignature)) {
    return ["trusted comment is not covered by the global signature"];
  }
  const findings: string[] = [];
  const fields = trustedCommentFields(signature.trustedComment);
  const quoted = (value: string | undefined): string => (value === undefined ? "none" : JSON.stringify(value));
  if (fields.get("file") !== fileName) {
    findings.push(`trusted comment names file ${quoted(fields.get("file"))}, not ${fileName}`);
  }
  // The updater's `requireSignedVersion` rejects a signature without it.
  if (fields.get("version") !== version) {
    findings.push(`trusted comment carries version ${quoted(fields.get("version"))}, not ${version}`);
  }
  return findings;
}

/** Minisign prints key IDs as the little-endian 64-bit number in hex. */
export function hexKeyId(keyId: Buffer): string {
  return Buffer.from(keyId).reverse().toString("hex").toUpperCase();
}
