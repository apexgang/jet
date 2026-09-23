import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const FEATURES = new URL("../src/lib/features/", import.meta.url).pathname;

function svelteFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return svelteFiles(path);
    return entry.name.endsWith(".svelte") ? [path] : [];
  });
}

describe("live views", () => {
  it("never read the desktop fixture scenario (D2)", () => {
    const files = svelteFiles(FEATURES);
    expect(files.length).toBeGreaterThan(0);
    const offenders = files
      .filter((file) => /\bscenario\./.test(readFileSync(file, "utf8")))
      .map((file) => relative(FEATURES, file));
    expect(offenders).toEqual([]);
  });
});
