import { gzipSync } from "node:zlib";
import { describe, expect, it } from "vitest";

import {
  ArchiveError,
  debEntries,
  normalizeEntryPath,
  readAr,
  readCpio,
  readTar,
  rpmEntries,
  rpmPayload,
} from "../scripts/release/archives";

// Minimal writers for the formats the release check reads, so the readers are
// tested without dpkg, rpm or bsdtar on the machine.

type TarOptions = { type?: string; prefix?: string; gnu?: boolean; data?: Buffer };

function tarBlock(name: string, { type = "0", prefix, gnu = false, data = Buffer.alloc(0) }: TarOptions = {}): Buffer {
  const header = Buffer.alloc(512);
  header.write(name, 0, 100, "utf8");
  header.write("0000644\0", 100, "latin1");
  header.write(`${data.length.toString(8).padStart(11, "0")}\0`, 124, "latin1");
  header.write(type, 156, "latin1");
  if (gnu) header.write("ustar  \0", 257, "latin1");
  else header.write("ustar\u000000", 257, "latin1");
  if (prefix !== undefined) header.write(prefix, 345, 155, "utf8");
  const padding = Buffer.alloc((512 - (data.length % 512)) % 512);
  return Buffer.concat([header, data, padding]);
}

function tar(...blocks: Buffer[]): Buffer {
  return Buffer.concat([...blocks, Buffer.alloc(1024)]);
}

function paxRecord(key: string, value: string): Buffer {
  const body = ` ${key}=${value}\n`;
  let length = body.length + 1;
  while (`${length}${body}`.length !== length) length += 1;
  return Buffer.from(`${length}${body}`, "utf8");
}

function ar(members: [string, Buffer][]): Buffer {
  const parts: Buffer[] = [Buffer.from("!<arch>\n", "latin1")];
  for (const [name, data] of members) {
    const header =
      name.padEnd(16) + "0".padEnd(12) + "0".padEnd(6) + "0".padEnd(6) + "100644".padEnd(8) + String(data.length).padEnd(10) + "`\n";
    parts.push(Buffer.from(header, "latin1"), data);
    if (data.length % 2 === 1) parts.push(Buffer.from("\n", "latin1"));
  }
  return Buffer.concat(parts);
}

function cpioEntry(name: string, mode: number, data = Buffer.alloc(0)): Buffer {
  const nameBytes = Buffer.from(`${name}\0`, "utf8");
  const fields = [0, mode, 0, 0, 1, 0, data.length, 0, 0, 0, 0, nameBytes.length, 0];
  const header = Buffer.from(`070701${fields.map((field) => field.toString(16).padStart(8, "0")).join("")}`, "latin1");
  const namePadding = Buffer.alloc((4 - ((header.length + nameBytes.length) % 4)) % 4);
  const dataPadding = Buffer.alloc((4 - (data.length % 4)) % 4);
  return Buffer.concat([header, nameBytes, namePadding, data, dataPadding]);
}

function cpio(...entries: Buffer[]): Buffer {
  return Buffer.concat([...entries, cpioEntry("TRAILER!!!", 0)]);
}

function rpmHeader(indexCount: number, store: Buffer): Buffer {
  const intro = Buffer.from([0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
  intro.writeUInt32BE(indexCount, 8);
  intro.writeUInt32BE(store.length, 12);
  return Buffer.concat([intro, Buffer.alloc(indexCount * 16), store]);
}

function rpm(payload: Buffer): Buffer {
  const lead = Buffer.alloc(96);
  Buffer.from([0xed, 0xab, 0xee, 0xdb]).copy(lead);
  const signature = rpmHeader(1, Buffer.from("12345")); // 37 bytes, padded to 40
  const main = rpmHeader(0, Buffer.from("abc")); // 19 bytes, not padded
  return Buffer.concat([lead, signature, Buffer.alloc(3), main, payload]);
}

describe("normalizeEntryPath", () => {
  it.each([
    ["./usr/lib/Jet/jet-core.tar.gz", "usr/lib/Jet/jet-core.tar.gz"],
    ["/usr/bin/jet-tauri", "usr/bin/jet-tauri"],
    ["./usr/", "usr"],
    ["./", ""],
    [".", ""],
  ])("normalizes %s", (input, expected) => {
    expect(normalizeEntryPath(input)).toBe(expected);
  });
});

describe("readTar", () => {
  it("reads files, directories, symlinks, ustar prefixes, GNU long names and pax paths", () => {
    const longName = `usr/share/${"a".repeat(120)}/file.txt`;
    const entries = readTar(
      tar(
        tarBlock("./usr/", { type: "5" }),
        tarBlock("./usr/bin/jet-tauri", { data: Buffer.from("binary") }),
        tarBlock("link", { type: "2" }),
        tarBlock("core.tar.gz", { prefix: "usr/lib/Jet", data: Buffer.from("payload") }),
        tarBlock("././@LongLink", { type: "L", gnu: true, data: Buffer.from(`${longName}\0`) }),
        tarBlock("truncated-name", { gnu: true, data: Buffer.from("long") }),
        tarBlock("PaxHeader", { type: "x", data: paxRecord("path", "usr/pax/named.txt") }),
        tarBlock("ignored", { data: Buffer.from("pax") }),
      ),
    );
    expect(entries.map((entry) => [entry.path, entry.kind, entry.data.toString()])).toEqual([
      ["usr", "directory", ""],
      ["usr/bin/jet-tauri", "file", "binary"],
      ["link", "symlink", ""],
      ["usr/lib/Jet/core.tar.gz", "file", "payload"],
      [longName, "file", "long"],
      ["usr/pax/named.txt", "file", "pax"],
    ]);
  });

  it("ignores the prefix bytes of a GNU header", () => {
    const [entry] = readTar(tar(tarBlock("plain", { gnu: true, prefix: "not-a-prefix", data: Buffer.from("x") })));
    expect(entry.path).toBe("plain");
  });

  it("refuses an entry that runs past the end", () => {
    const whole = tar(tarBlock("big", { data: Buffer.alloc(2048, 1) }));
    expect(() => readTar(whole.subarray(0, 1024))).toThrow(ArchiveError);
  });

  it("refuses a malformed size", () => {
    const block = tarBlock("bad", { data: Buffer.from("x") });
    block.write("zz", 124, "latin1");
    expect(() => readTar(tar(block))).toThrow(/bad size/);
  });
});

describe("readAr", () => {
  it("reads members with odd sizes and GNU trailing slashes", () => {
    const members = readAr(
      ar([
        ["debian-binary", Buffer.from("2.0\n")],
        ["odd/", Buffer.from("abc")],
        ["after", Buffer.from("z")],
      ]),
    );
    expect([...members].map(([name, data]) => [name, data.toString()])).toEqual([
      ["debian-binary", "2.0\n"],
      ["odd", "abc"],
      ["after", "z"],
    ]);
  });

  it("refuses a file without the ar magic or with a truncated member", () => {
    expect(() => readAr(Buffer.from("not an archive"))).toThrow(ArchiveError);
    const whole = ar([["member", Buffer.alloc(100)]]);
    expect(() => readAr(whole.subarray(0, whole.length - 10))).toThrow(/truncated/);
  });
});

describe("readCpio", () => {
  it("reads regular files, directories and symlinks up to the trailer", () => {
    const entries = readCpio(
      cpio(
        cpioEntry("./usr", 0o040755),
        cpioEntry("./usr/lib/Jet/jet-core.tar.gz", 0o100644, Buffer.from("payload")),
        cpioEntry("./usr/bin/link", 0o120777, Buffer.from("target")),
      ),
    );
    expect(entries.map((entry) => [entry.path, entry.kind, entry.data.toString()])).toEqual([
      ["usr", "directory", ""],
      ["usr/lib/Jet/jet-core.tar.gz", "file", "payload"],
      ["usr/bin/link", "symlink", ""],
    ]);
  });

  it("refuses an unknown magic and a missing trailer", () => {
    expect(() => readCpio(Buffer.from("070707".padEnd(110, "0")))).toThrow(/magic/);
    expect(() => readCpio(cpioEntry("file", 0o100644, Buffer.from("x")))).toThrow(ArchiveError);
  });
});

describe("packages", () => {
  const payloadArchive = Buffer.from("the core payload archive");

  it("reads the files of a deb's data.tar.gz", () => {
    const data = gzipSync(tar(tarBlock("./usr/lib/Jet/jet-core.tar.gz", { data: payloadArchive })));
    const deb = ar([
      ["debian-binary", Buffer.from("2.0\n")],
      ["control.tar.gz", gzipSync(tar())],
      ["data.tar.gz", data],
    ]);
    const [entry] = debEntries(deb);
    expect(entry.path).toBe("usr/lib/Jet/jet-core.tar.gz");
    expect(entry.data.equals(payloadArchive)).toBe(true);
  });

  it("names the members of a deb without data.tar.gz", () => {
    expect(() => debEntries(ar([["data.tar.xz", Buffer.from("x")]]))).toThrow(/data\.tar\.xz/);
  });

  it("skips the rpm lead, the padded signature header and the main header", () => {
    const payload = gzipSync(cpio(cpioEntry("./usr/lib/Jet/jet-core.tar.gz", 0o100644, payloadArchive)));
    const package_ = rpm(payload);
    expect(rpmPayload(package_).equals(payload)).toBe(true);
    const [entry] = rpmEntries(package_);
    expect(entry.path).toBe("usr/lib/Jet/jet-core.tar.gz");
    expect(entry.data.equals(payloadArchive)).toBe(true);
  });

  it("refuses an rpm with a bad magic or a payload that is not gzip", () => {
    expect(() => rpmPayload(Buffer.alloc(200))).toThrow(/lead magic/);
    expect(() => rpmEntries(rpm(Buffer.from("zstd?")))).toThrow(/not gzip/);
  });
});
