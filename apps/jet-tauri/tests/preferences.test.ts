import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { publicError } from "../src/lib/jet/errors";
import { loadDesktopPreferences, setDesktopPreferences, type DesktopPreferences } from "../src/lib/jet/preferences";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

describe("desktop preferences adapter", () => {
  it("round-trips reopenLastTask", async () => {
    let stored: DesktopPreferences = { reopenLastTask: true };
    const calls: Array<{ command: string; args: unknown }> = [];
    vi.stubGlobal("window", {});
    mockIPC((command, args) => {
      calls.push({ command, args });
      if (command === "load_desktop_preferences") return stored;
      if (command === "set_desktop_preferences") {
        stored = (args as { preferences: DesktopPreferences }).preferences;
        return stored;
      }
      throw new Error(`Unexpected ${command}`);
    });
    expect(await loadDesktopPreferences()).toEqual({ reopenLastTask: true });
    expect(await setDesktopPreferences({ reopenLastTask: false })).toEqual({ reopenLastTask: false });
    expect(await loadDesktopPreferences()).toEqual({ reopenLastTask: false });
    expect(calls[1]).toEqual({ command: "set_desktop_preferences", args: { preferences: { reopenLastTask: false } } });
  });

  it("surfaces a refusal as a PublicError", async () => {
    vi.stubGlobal("window", {});
    mockIPC(() => {
      throw { category: "invalid_input", code: "preferences.invalid", message: "Those desktop preferences are not valid.", retryable: false };
    });
    const failure = await setDesktopPreferences({ reopenLastTask: true }).then(
      () => {
        throw new Error("expected a refusal");
      },
      (error: unknown) => publicError(error),
    );
    expect(failure.code).toBe("preferences.invalid");
  });
});
