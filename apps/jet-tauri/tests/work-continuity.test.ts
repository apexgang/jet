import { describe, expect, test } from "vitest";

import { shouldLoadNextWorkPage } from "../src/lib/jet/work-continuity";

describe("work refresh continuity", () => {
  test("keeps paging when an insertion pushes the selected file beyond the old page count", () => {
    const selected = "file-zeta";
    const insertedFirstPage = new Set(["file-alpha", "file-beta"]);

    expect(shouldLoadNextWorkPage(insertedFirstPage, 2, selected, true)).toBe(true);
    expect(
      shouldLoadNextWorkPage(new Set([...insertedFirstPage, selected]), 2, selected, true),
    ).toBe(false);
  });
});
