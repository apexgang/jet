import { describe, expect, it } from "vitest";

import { publicError } from "../src/lib/features/shell/session.svelte";

describe("setup error normalization", () => {
  it("preserves a structured stable error", () => {
    expect(
      publicError({
        category: "offline",
        code: "transport.offline",
        message: "Jet could not reach this Plane.",
        retryable: true,
      }),
    ).toEqual({
      category: "offline",
      code: "transport.offline",
      message: "Jet could not reach this Plane.",
      retryable: true,
    });
  });

  it("accepts the serialized error shape returned by desktop IPC", () => {
    expect(
      publicError(
        JSON.stringify({
          category: "offline",
          code: "transport.offline",
          message: "Jet could not reach this Plane.",
          retryable: true,
        }),
      ),
    ).toMatchObject({ category: "offline", code: "transport.offline", retryable: true });
  });

  it("does not expose unstructured native failures", () => {
    expect(publicError("connection refused at /private/user/path")).toEqual({
      category: "internal",
      code: "client.request_failed",
      message: "Jet could not complete the request.",
      retryable: false,
    });
  });
});
