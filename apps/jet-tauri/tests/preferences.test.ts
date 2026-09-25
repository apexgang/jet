import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { publicError } from "../src/lib/jet/errors";
import {
  loadDesktopPreferences,
  setDesktopPreferences,
  type DesktopPreferences,
  type DesktopPreferencesChange,
} from "../src/lib/jet/preferences";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

describe("desktop preferences adapter", () => {
  it("sends only the changed preference and resolves with all of them", async () => {
    let stored: DesktopPreferences = { reopenLastTask: true, checkForUpdates: true };
    const calls: Array<{ command: string; args: unknown }> = [];
    vi.stubGlobal("window", {});
    mockIPC((command, args) => {
      calls.push({ command, args });
      if (command === "load_desktop_preferences") return stored;
      if (command === "set_desktop_preferences") {
        // The shell keeps a preference the change leaves out.
        stored = { ...stored, ...(args as { preferences: DesktopPreferencesChange }).preferences };
        return stored;
      }
      throw new Error(`Unexpected ${command}`);
    });
    expect(await loadDesktopPreferences()).toEqual({ reopenLastTask: true, checkForUpdates: true });
    expect(await setDesktopPreferences({ checkForUpdates: false })).toEqual({
      reopenLastTask: true,
      checkForUpdates: false,
    });
    expect(await setDesktopPreferences({ reopenLastTask: false })).toEqual({
      reopenLastTask: false,
      checkForUpdates: false,
    });
    expect(calls.slice(1)).toEqual([
      { command: "set_desktop_preferences", args: { preferences: { checkForUpdates: false } } },
      { command: "set_desktop_preferences", args: { preferences: { reopenLastTask: false } } },
    ]);
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
