import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import { publicError } from "../src/lib/jet/errors";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

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
      recoveryActions: [],
      restart: null,
      revisionConflict: null,
      protocolLimit: null,
      planeId: null,
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
      recoveryActions: [],
      restart: null,
      revisionConflict: null,
      protocolLimit: null,
      planeId: null,
    });
  });
});

describe("setup and Planes destinations", () => {
  it("Planes opens the Planes destination instead of a placeholder, and Setup links to Add a Plane", async () => {
    const calls: string[] = [];
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command) => {
      calls.push(command);
      throw { category: "offline", code: "transport.offline", message: "offline", retryable: true };
    });
    const session = new DesktopSession();

    session.select("planes");
    expect(session.sidebarSelection).toBe("planes");
    expect(session.actionNotice ?? "").not.toMatch(/planned for Wave 3/);

    session.select("project");
    session.openAddPlane();
    expect(session.sidebarSelection).toBe("planes");
    expect(session.planes.focusRequest?.section).toBe("add");
    await Promise.resolve();
    expect(calls).toContain("list_planes");
  });
});
