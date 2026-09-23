import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { publicError } from "../src/lib/jet/errors";
import {
  loadNotificationSettings,
  setNotificationSettings,
  type NotificationPreferences,
  type NotificationSettings,
} from "../src/lib/jet/notifications";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

const remote = "0f8fad5b-d9cb-469f-a165-70867728950e";
const planes = [
  { planeId: "local", label: "This computer" },
  { planeId: remote, label: "build.example" },
];

describe("notification settings adapter", () => {
  it("round-trips the v2 shape with mutedPlanes", async () => {
    let stored: NotificationPreferences = {
      enabled: false,
      approvals: true,
      completion: true,
      failure: true,
      mutedPlanes: [],
    };
    const calls: Array<{ command: string; args: unknown }> = [];
    vi.stubGlobal("window", {});
    mockIPC((command, args) => {
      calls.push({ command, args });
      const settings = (): NotificationSettings => ({ preferences: stored, error: null, planes });
      if (command === "load_notification_settings") return settings();
      if (command === "set_notification_settings") {
        stored = (args as { preferences: NotificationPreferences }).preferences;
        return settings();
      }
      throw new Error(`Unexpected ${command}`);
    });
    const initial = await loadNotificationSettings();
    expect(initial.planes).toEqual(planes);
    expect(initial.preferences.mutedPlanes).toEqual([]);

    const next: NotificationPreferences = { ...initial.preferences, enabled: true, mutedPlanes: [remote] };
    const saved = await setNotificationSettings(next);
    expect(saved.preferences).toEqual(next);
    expect((await loadNotificationSettings()).preferences.mutedPlanes).toEqual([remote]);
    expect(calls[0]).toEqual({ command: "load_notification_settings", args: {} });
    expect(calls[1]).toEqual({
      command: "set_notification_settings",
      args: {
        preferences: {
          enabled: true,
          approvals: true,
          completion: true,
          failure: true,
          mutedPlanes: [remote],
        },
      },
    });
  });

  it("surfaces a refusal as a PublicError", async () => {
    vi.stubGlobal("window", {});
    mockIPC(() => {
      throw {
        category: "invalid_input",
        code: "notifications.permission_denied",
        message: "Allow Jet notifications in your desktop settings before enabling them.",
        retryable: false,
      };
    });
    const failure = await setNotificationSettings({
      enabled: true,
      approvals: true,
      completion: true,
      failure: true,
      mutedPlanes: [],
    }).then(
      () => {
        throw new Error("expected a refusal");
      },
      (error: unknown) => publicError(error),
    );
    expect(failure.code).toBe("notifications.permission_denied");
  });
});
