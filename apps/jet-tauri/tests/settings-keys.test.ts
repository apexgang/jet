import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { handleSettingsKey, settingsKeyAction } from "../src/lib/features/settings/keys";
import type { KeyInput } from "../src/lib/features/shell/shortcuts";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function press(input: Partial<KeyInput> & { key: string }) {
  const prevented = { value: false };
  const event = {
    code: "",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    repeat: false,
    ...input,
    preventDefault: () => (prevented.value = true),
  };
  return { event, prevented };
}

async function flush() {
  for (let tick = 0; tick < 10; tick += 1) await Promise.resolve();
}

describe("Settings window shortcuts", () => {
  it("resolves Ctrl+W, Ctrl+Q and Ctrl+, like the main window", () => {
    expect(settingsKeyAction(press({ key: "w", code: "KeyW", ctrlKey: true }).event, "other")).toBe("close");
    expect(settingsKeyAction(press({ key: "q", code: "KeyQ", ctrlKey: true }).event, "other")).toBe("quit");
    expect(settingsKeyAction(press({ key: ",", code: "Comma", ctrlKey: true }).event, "other")).toBe("focus-navigation");
    expect(settingsKeyAction(press({ key: "w", code: "KeyW", metaKey: true }).event, "mac")).toBe("close");
  });

  it("uses the physical key on non-Latin layouts", () => {
    expect(settingsKeyAction(press({ key: "й", code: "KeyQ", ctrlKey: true }).event, "other")).toBe("quit");
    expect(settingsKeyAction(press({ key: "ц", code: "KeyW", ctrlKey: true }).event, "other")).toBe("close");
    expect(settingsKeyAction(press({ key: "б", code: "Comma", ctrlKey: true }).event, "other")).toBe("focus-navigation");
  });

  it("leaves other presses alone: main-window intents, repeats, composition, extra modifiers", () => {
    for (const input of [
      { key: "n", code: "KeyN", ctrlKey: true },
      { key: "F11", code: "F11" },
      { key: "q", code: "KeyQ", ctrlKey: true, repeat: true },
      { key: "q", code: "KeyQ", ctrlKey: true, isComposing: true },
      { key: "Q", code: "KeyQ", ctrlKey: true, shiftKey: true },
      { key: "q", code: "KeyQ", ctrlKey: true, altKey: true },
      { key: "q", code: "KeyQ" },
    ]) {
      expect(settingsKeyAction(press(input).event, "other"), JSON.stringify(input)).toBeNull();
    }
  });

  it("Ctrl+Q on a Cyrillic layout calls quit_jet", async () => {
    const commands: string[] = [];
    vi.stubGlobal("window", {});
    mockIPC((command) => {
      commands.push(command);
      return null;
    });
    const focusNavigation = vi.fn();
    const quit = press({ key: "й", code: "KeyQ", ctrlKey: true });
    handleSettingsKey(quit.event, "other", focusNavigation);
    const close = press({ key: "ц", code: "KeyW", ctrlKey: true });
    handleSettingsKey(close.event, "other", focusNavigation);
    const typing = press({ key: "й", code: "KeyQ" });
    handleSettingsKey(typing.event, "other", focusNavigation);
    await flush();
    expect(commands).toEqual(["quit_jet", "close_settings"]);
    expect([quit.prevented.value, close.prevented.value, typing.prevented.value]).toEqual([true, true, false]);
    expect(focusNavigation).not.toHaveBeenCalled();
  });
});
