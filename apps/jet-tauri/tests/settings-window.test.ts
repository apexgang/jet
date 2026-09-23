import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import { publicError } from "../src/lib/jet/errors";
import {
  closeSettings,
  openSettings,
  rememberSettingsPane,
  watchSettingsNavigation,
  type SettingsNavigation,
} from "../src/lib/jet/settings-window";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function ipc(handler: (command: string, args: Record<string, unknown>) => unknown) {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC((command, args) => {
    const record = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: record });
    return handler(command, record);
  });
  return calls;
}

describe("settings window adapter", () => {
  it("opens Settings with a typed target whose Plane field stays snake_case", async () => {
    const calls = ipc(() => null);
    await openSettings({ pane: "connections", section: "local_service", plane_id: "local" });
    await openSettings();
    expect(calls).toEqual([
      { command: "open_settings", args: { target: { pane: "connections", section: "local_service", plane_id: "local" } } },
      { command: "open_settings", args: { target: null } },
    ]);
  });

  it("passes a channel and returns the initial navigation", async () => {
    let channel: Channel<SettingsNavigation> | null = null;
    const initial: SettingsNavigation = { generation: "1", target: { pane: "general", section: "notifications", plane_id: null } };
    const calls = ipc((command, args) => {
      if (command !== "watch_settings_navigation") throw new Error(`Unexpected ${command}`);
      channel = args.onNavigate as Channel<SettingsNavigation>;
      return initial;
    });
    const received: SettingsNavigation[] = [];
    const navigation = await watchSettingsNavigation((message) => received.push(message));
    expect(navigation).toEqual(initial);
    expect(calls.map((call) => Object.keys(call.args))).toEqual([["onNavigate"]]);
    expect(channel).not.toBeNull();
    const next: SettingsNavigation = { generation: "1", target: { pane: "connections", section: "planes", plane_id: null } };
    channel!.onmessage(next);
    expect(received).toEqual([next]);
  });

  it("remembers a pane and closes through fixed commands", async () => {
    const calls = ipc(() => null);
    await rememberSettingsPane({ pane: "connections" });
    await closeSettings();
    expect(calls).toEqual([
      { command: "remember_settings_pane", args: { target: { pane: "connections" } } },
      { command: "close_settings", args: {} },
    ]);
  });

  it("keeps a thrown PublicError intact", async () => {
    const refusal = { category: "invalid_input", code: "settings.target_invalid", message: "That Settings location does not exist.", retryable: false };
    ipc(() => { throw refusal; });
    const failure = await openSettings({ pane: "general", section: "accounts" }).catch((error: unknown) => publicError(error));
    expect(failure).toMatchObject(refusal);
  });
});
