import { describe, expect, test } from "vitest";

import { TerminalTranscriptDecoder } from "../src/lib/jet/terminal-text";

describe("terminal transcript decoding", () => {
  test("joins UTF-8 split across protocol frames", () => {
    const decoder = new TerminalTranscriptDecoder();
    expect(decoder.push([0x68, 0x69, 0x20, 0xf0, 0x9f])).toBe("hi ");
    expect(decoder.push([0x91, 0x8b, 0x0a])).toBe("👋\n");
    expect(decoder.finish()).toBe("");
  });

  test("removes ANSI and OSC sequences split across frames", () => {
    const decoder = new TerminalTranscriptDecoder();
    expect(decoder.push(Array.from(new TextEncoder().encode("ok\u001b[31")))).toBe("ok");
    expect(decoder.push(Array.from(new TextEncoder().encode("mred\u001b[0m\u001b]0;title")))).toBe("red");
    expect(decoder.push([0x07, 0x0a])).toBe("\n");
  });
});
