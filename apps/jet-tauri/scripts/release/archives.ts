/**
 * Readers for the containers Tauri writes on Linux, so `just release-verify`
 * can open a deb or rpm without `dpkg-deb`, `rpm2cpio` or `bsdtar` on the
 * host: ar (deb), tar (deb data, core payload), cpio newc (rpm payload) and
 * the rpm lead and headers. Each reader takes the whole container in memory
 * and returns its members; offsets and sizes come from untrusted bytes, so
 * every read is bounds-checked and a malformed container throws
 * `ArchiveError` instead of reading past the buffer.
 */

import { gunzipSync } from "node:zlib";

export class ArchiveError extends Error {}

export type EntryKind = "file" | "directory" | "symlink" | "other";

export type ArchiveEntry = {
  /** Relative path without a leading `./` or `/`, never ending in `/`. */
  path: string;
  kind: EntryKind;
  /** File content; empty for anything but a regular file. */
  data: Buffer;
};

/** Strips the `./`, `/` and trailing `/` spellings archives use for one path. */
export function normalizeEntryPath(path: string): string {
  let result = path;
  for (;;) {
    if (result.startsWith("./")) result = result.slice(2);
    else if (result.startsWith("/")) result = result.slice(1);
    else break;
  }
  while (result.endsWith("/")) result = result.slice(0, -1);
  return result === "." ? "" : result;
}

function slice(buffer: Buffer, start: number, length: number, what: string): Buffer {
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(length) || start < 0 || length < 0) {
    throw new ArchiveError(`${what}: invalid offset`);
  }
  if (start + length > buffer.length) {
    throw new ArchiveError(`${what}: truncated at byte ${buffer.length}`);
  }
  return buffer.subarray(start, start + length);
}

function cString(field: Buffer): string {
  const end = field.indexOf(0);
  return field.subarray(0, end === -1 ? field.length : end).toString("utf8");
}

// ---------------------------------------------------------------------------
// ar (the outer container of a .deb)

const AR_MAGIC = "!<arch>\n";
const AR_HEADER = 60;

/** Members of a common-format ar archive by name, as `dpkg-deb` reads them. */
export function readAr(buffer: Buffer): Map<string, Buffer> {
  if (buffer.subarray(0, AR_MAGIC.length).toString("latin1") !== AR_MAGIC) {
    throw new ArchiveError("ar: missing !<arch> magic");
  }
  const members = new Map<string, Buffer>();
  let offset = AR_MAGIC.length;
  while (offset < buffer.length) {
    const header = slice(buffer, offset, AR_HEADER, "ar header");
    if (header.subarray(58, 60).toString("latin1") !== "`\n") {
      throw new ArchiveError(`ar: bad header terminator at byte ${offset}`);
    }
    const name = header.subarray(0, 16).toString("latin1").trimEnd().replace(/\/$/, "");
    const sizeText = header.subarray(48, 58).toString("latin1").trim();
    if (!/^\d+$/.test(sizeText)) throw new ArchiveError(`ar: bad size for ${name}`);
    const size = Number(sizeText);
    members.set(name, slice(buffer, offset + AR_HEADER, size, `ar member ${name}`));
    offset += AR_HEADER + size + (size % 2);
  }
  return members;
}

// ---------------------------------------------------------------------------
// tar (ustar, GNU long names and PAX paths)

const BLOCK = 512;

function tarNumber(field: Buffer, what: string): number {
  // GNU base-256: the high bit of the first byte marks a big-endian binary number.
  if (field.length > 0 && (field[0] & 0x80) !== 0) {
    let value = field[0] & 0x7f;
    for (const byte of field.subarray(1)) {
      value = value * 256 + byte;
      if (!Number.isSafeInteger(value)) throw new ArchiveError(`tar: ${what} too large`);
    }
    return value;
  }
  const text = cString(field).trim();
  if (text === "") return 0;
  if (!/^[0-7]+$/.test(text)) throw new ArchiveError(`tar: bad ${what} ${JSON.stringify(text)}`);
  return parseInt(text, 8);
}

function paxRecords(data: Buffer): Map<string, string> {
  const records = new Map<string, string>();
  let offset = 0;
  while (offset < data.length) {
    const space = data.indexOf(0x20, offset);
    if (space === -1) throw new ArchiveError("tar: bad pax record");
    const length = Number(data.subarray(offset, space).toString("latin1"));
    if (!Number.isSafeInteger(length) || length <= 0 || offset + length > data.length) {
      throw new ArchiveError("tar: bad pax record length");
    }
    const record = data.subarray(space + 1, offset + length - 1).toString("utf8");
    const equals = record.indexOf("=");
    if (equals !== -1) records.set(record.slice(0, equals), record.slice(equals + 1));
    offset += length;
  }
  return records;
}

function isZeroBlock(block: Buffer): boolean {
  return block.every((byte) => byte === 0);
}

/** Entries of an uncompressed tar stream. */
export function readTar(buffer: Buffer): ArchiveEntry[] {
  const entries: ArchiveEntry[] = [];
  let offset = 0;
  let longName: string | undefined;
  let pax = new Map<string, string>();
  while (offset + BLOCK <= buffer.length) {
    const header = buffer.subarray(offset, offset + BLOCK);
    if (isZeroBlock(header)) break;
    const typeflag = String.fromCharCode(header[156]);
    const size = tarNumber(header.subarray(124, 136), "size");
    const paxSize = pax.get("size");
    const dataSize = paxSize === undefined ? size : Number(paxSize);
    if (!Number.isSafeInteger(dataSize) || dataSize < 0) throw new ArchiveError("tar: bad pax size");
    const data = slice(buffer, offset + BLOCK, dataSize, "tar entry");
    offset += BLOCK + Math.ceil(dataSize / BLOCK) * BLOCK;

    if (typeflag === "L") {
      longName = cString(data);
      continue;
    }
    if (typeflag === "x") {
      pax = paxRecords(data);
      continue;
    }
    if (typeflag === "g" || typeflag === "K") continue;

    // Only POSIX ustar has a name prefix; GNU headers ("ustar  ") keep
    // times and sparse data in the same bytes and use `L` entries instead.
    const posix = header.subarray(257, 263).toString("latin1") === "ustar\0";
    const name = cString(header.subarray(0, 100));
    const prefix = posix ? cString(header.subarray(345, 500)) : "";
    const path = pax.get("path") ?? longName ?? (prefix ? `${prefix}/${name}` : name);
    longName = undefined;
    pax = new Map();

    const kind: EntryKind =
      typeflag === "0" || typeflag === "\0" || typeflag === "7"
        ? "file"
        : typeflag === "5"
          ? "directory"
          : typeflag === "2"
            ? "symlink"
            : "other";
    entries.push({ path: normalizeEntryPath(path), kind, data: kind === "file" ? data : Buffer.alloc(0) });
  }
  return entries;
}

// ---------------------------------------------------------------------------
// cpio newc (the payload of an rpm)

const CPIO_HEADER = 110;

function align4(value: number): number {
  return Math.ceil(value / 4) * 4;
}

/** Entries of an uncompressed cpio archive in the `newc` or `crc` format. */
export function readCpio(buffer: Buffer): ArchiveEntry[] {
  const entries: ArchiveEntry[] = [];
  let offset = 0;
  for (;;) {
    const header = slice(buffer, offset, CPIO_HEADER, "cpio header").toString("latin1");
    const magic = header.slice(0, 6);
    if (magic !== "070701" && magic !== "070702") {
      throw new ArchiveError(`cpio: unsupported magic ${JSON.stringify(magic)} at byte ${offset}`);
    }
    const field = (index: number): number => {
      const text = header.slice(6 + index * 8, 14 + index * 8);
      if (!/^[0-9a-fA-F]{8}$/.test(text)) throw new ArchiveError("cpio: bad header field");
      return parseInt(text, 16);
    };
    const mode = field(1);
    const fileSize = field(6);
    const nameSize = field(11);
    const name = cString(slice(buffer, offset + CPIO_HEADER, nameSize, "cpio name"));
    const dataStart = align4(offset + CPIO_HEADER + nameSize);
    const data = slice(buffer, dataStart, fileSize, `cpio entry ${name}`);
    offset = align4(dataStart + fileSize);
    if (name === "TRAILER!!!") break;

    const type = mode & 0o170000;
    const kind: EntryKind =
      type === 0o100000 ? "file" : type === 0o040000 ? "directory" : type === 0o120000 ? "symlink" : "other";
    entries.push({ path: normalizeEntryPath(name), kind, data: kind === "file" ? data : Buffer.alloc(0) });
  }
  return entries;
}

// ---------------------------------------------------------------------------
// rpm

const RPM_LEAD = 96;
const RPM_LEAD_MAGIC = Buffer.from([0xed, 0xab, 0xee, 0xdb]);
const RPM_HEADER_MAGIC = Buffer.from([0x8e, 0xad, 0xe8, 0x01]);

function rpmHeaderLength(buffer: Buffer, offset: number, what: string): number {
  const intro = slice(buffer, offset, 16, what);
  if (!intro.subarray(0, 4).equals(RPM_HEADER_MAGIC)) throw new ArchiveError(`rpm: bad ${what} magic`);
  const indexCount = intro.readUInt32BE(8);
  const storeSize = intro.readUInt32BE(12);
  return 16 + indexCount * 16 + storeSize;
}

/** The compressed cpio payload that follows an rpm's lead, signature and header. */
export function rpmPayload(buffer: Buffer): Buffer {
  if (!slice(buffer, 0, 4, "rpm lead").equals(RPM_LEAD_MAGIC)) throw new ArchiveError("rpm: bad lead magic");
  let offset = RPM_LEAD;
  const signature = rpmHeaderLength(buffer, offset, "signature header");
  // The signature header is padded to an 8-byte boundary; the main header is not.
  offset += signature + ((8 - (signature % 8)) % 8);
  offset += rpmHeaderLength(buffer, offset, "main header");
  return slice(buffer, offset, buffer.length - offset, "rpm payload");
}

// ---------------------------------------------------------------------------
// Compression and whole packages

const GZIP_MAGIC = Buffer.from([0x1f, 0x8b]);

/**
 * Inflates a gzip stream. Tauri writes deb data and, unless
 * `bundle.linux.rpm.compression` says otherwise, rpm payloads with gzip.
 */
export function gunzip(buffer: Buffer, what: string): Buffer {
  if (!buffer.subarray(0, 2).equals(GZIP_MAGIC)) throw new ArchiveError(`${what}: not gzip-compressed`);
  try {
    return gunzipSync(buffer);
  } catch (error) {
    throw new ArchiveError(`${what}: ${(error as Error).message}`);
  }
}

/** Files of a .deb's `data.tar.gz`. */
export function debEntries(deb: Buffer): ArchiveEntry[] {
  const members = readAr(deb);
  const data = members.get("data.tar.gz");
  if (data === undefined) {
    throw new ArchiveError(`deb: no data.tar.gz member (found ${[...members.keys()].join(", ")})`);
  }
  return readTar(gunzip(data, "deb data.tar.gz"));
}

/** Files of an .rpm's cpio payload. */
export function rpmEntries(rpm: Buffer): ArchiveEntry[] {
  return readCpio(gunzip(rpmPayload(rpm), "rpm payload"));
}
