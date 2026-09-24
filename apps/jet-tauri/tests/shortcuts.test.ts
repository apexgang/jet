import { describe, expect, it } from "vitest";

import {
  ENABLED_SHORTCUTS,
  SHORTCUTS,
  WORK_PANEL_TAB_ORDER,
  detectPlatform,
  resolveShortcut,
  shortcutAria,
  shortcutLabel,
  type KeyInput,
  type Platform,
  type ShellIntentKind,
  type ShortcutContext,
} from "../src/lib/features/shell/shortcuts";

const ALL: ReadonlySet<ShellIntentKind> = new Set<ShellIntentKind>([
  "new-task",
  "search",
  "add-project",
  "settings",
  "toggle-sidebar",
  "toggle-work-panel",
  "work-panel-tab",
  "toggle-fullscreen",
  "close-window",
  "quit",
  "dismiss",
]);

function key(input: Partial<KeyInput> & { key: string }): KeyInput {
  return {
    code: "",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    repeat: false,
    ...input,
  };
}

function context(platform: Platform, extra: Partial<ShortcutContext> = {}): ShortcutContext {
  return { platform, modalOpen: false, dismissible: true, enabled: ALL, ...extra };
}

const letter = (character: string) => ({ key: character, code: `Key${character.toUpperCase()}` });
const digit = (value: number) => ({ key: String(value), code: `Digit${value}` });

describe("resolveShortcut bindings", () => {
  const linux: Array<[string, KeyInput, unknown]> = [
    ["Ctrl+N", key({ ...letter("n"), ctrlKey: true }), { kind: "new-task" }],
    ["Ctrl+K", key({ ...letter("k"), ctrlKey: true }), { kind: "search" }],
    ["Ctrl+Shift+O", key({ key: "O", code: "KeyO", ctrlKey: true, shiftKey: true }), { kind: "add-project" }],
    ["Ctrl+,", key({ key: ",", code: "Comma", ctrlKey: true }), { kind: "settings" }],
    ["F9", key({ key: "F9", code: "F9" }), { kind: "toggle-sidebar" }],
    ["Ctrl+Alt+0", key({ ...digit(0), ctrlKey: true, altKey: true }), { kind: "toggle-work-panel" }],
    ["Ctrl+Alt+1", key({ ...digit(1), ctrlKey: true, altKey: true }), { kind: "work-panel-tab", tab: "changes" }],
    ["Ctrl+Alt+2", key({ ...digit(2), ctrlKey: true, altKey: true }), { kind: "work-panel-tab", tab: "files" }],
    ["Ctrl+Alt+3", key({ ...digit(3), ctrlKey: true, altKey: true }), { kind: "work-panel-tab", tab: "terminal" }],
    ["Ctrl+Alt+4", key({ ...digit(4), ctrlKey: true, altKey: true }), { kind: "work-panel-tab", tab: "run" }],
    ["Ctrl+Alt+5", key({ ...digit(5), ctrlKey: true, altKey: true }), { kind: "work-panel-tab", tab: "delivery" }],
    ["Escape", key({ key: "Escape", code: "Escape" }), { kind: "dismiss" }],
    ["F11", key({ key: "F11", code: "F11" }), { kind: "toggle-fullscreen" }],
    ["Ctrl+W", key({ ...letter("w"), ctrlKey: true }), { kind: "close-window" }],
    ["Ctrl+Q", key({ ...letter("q"), ctrlKey: true }), { kind: "quit" }],
  ];

  it.each(linux)("Linux %s", (_, input, intent) => {
    expect(resolveShortcut(input, context("other"))).toEqual(intent);
  });

  const mac: Array<[string, KeyInput, unknown]> = [
    ["⌘N", key({ ...letter("n"), metaKey: true }), { kind: "new-task" }],
    ["⌘K", key({ ...letter("k"), metaKey: true }), { kind: "search" }],
    ["⇧⌘O", key({ key: "O", code: "KeyO", metaKey: true, shiftKey: true }), { kind: "add-project" }],
    ["⌘,", key({ key: ",", code: "Comma", metaKey: true }), { kind: "settings" }],
    ["⌃⌘S", key({ ...letter("s"), metaKey: true, ctrlKey: true }), { kind: "toggle-sidebar" }],
    ["⌥⌘0", key({ key: "º", code: "Digit0", metaKey: true, altKey: true }), { kind: "toggle-work-panel" }],
    ["⌥⌘4", key({ key: "¢", code: "Digit4", metaKey: true, altKey: true }), { kind: "work-panel-tab", tab: "run" }],
    ["Escape", key({ key: "Escape", code: "Escape" }), { kind: "dismiss" }],
    ["⌃⌘F", key({ ...letter("f"), metaKey: true, ctrlKey: true }), { kind: "toggle-fullscreen" }],
    ["⌘W", key({ ...letter("w"), metaKey: true }), { kind: "close-window" }],
    ["⌘Q", key({ ...letter("q"), metaKey: true }), { kind: "quit" }],
  ];

  it.each(mac)("macOS %s", (_, input, intent) => {
    expect(resolveShortcut(input, context("mac"))).toEqual(intent);
  });

  it("uses the platform's primary modifier only", () => {
    expect(resolveShortcut(key({ ...letter("n"), metaKey: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...letter("n"), ctrlKey: true }), context("mac"))).toBeNull();
    expect(resolveShortcut(key({ key: "F9", code: "F9" }), context("mac"))).toBeNull();
  });

  it("requires the exact modifiers", () => {
    expect(resolveShortcut(key({ ...letter("n"), ctrlKey: true, shiftKey: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...letter("k"), ctrlKey: true, altKey: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...letter("o"), ctrlKey: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ key: ",", code: "Comma", ctrlKey: true, shiftKey: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...digit(1), ctrlKey: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...digit(6), ctrlKey: true, altKey: true }), context("other"))).toBeNull();
  });

  it("matches letters by physical key on non-Latin layouts", () => {
    expect(resolveShortcut(key({ key: "т", code: "KeyN", ctrlKey: true }), context("other"))).toEqual({ kind: "new-task" });
    expect(resolveShortcut(key({ key: "л", code: "KeyK", ctrlKey: true }), context("other"))).toEqual({ kind: "search" });
    expect(resolveShortcut(key({ key: "б", code: "Comma", ctrlKey: true }), context("other"))).toEqual({ kind: "settings" });
  });

  it("matches digits by code, since Alt changes the key on some layouts", () => {
    expect(resolveShortcut(key({ key: "¡", code: "Digit1", ctrlKey: true, altKey: true }), context("other"))).toEqual({
      kind: "work-panel-tab",
      tab: "changes",
    });
    // A numpad digit is not a top-row digit.
    expect(resolveShortcut(key({ key: "1", code: "Numpad1", ctrlKey: true, altKey: true }), context("other"))).toBeNull();
  });

  it("Ctrl+, keeps opening Settings (Wave 3.2)", () => {
    expect(resolveShortcut(key({ key: ",", code: "Comma", ctrlKey: true }), context("other", { enabled: ENABLED_SHORTCUTS }))).toEqual({
      kind: "settings",
    });
  });
});

describe("resolveShortcut guards", () => {
  const ctrlN = key({ ...letter("n"), ctrlKey: true });

  it("ignores key presses while an input method composes", () => {
    expect(resolveShortcut({ ...ctrlN, isComposing: true }, context("other"))).toBeNull();
    expect(resolveShortcut(key({ key: "Escape", code: "Escape", isComposing: true }), context("other"))).toBeNull();
  });

  it("ignores everything while a modal dialog is open, Escape included (D4)", () => {
    for (const input of [ctrlN, key({ key: "Escape", code: "Escape" }), key({ key: "F9", code: "F9" })]) {
      expect(resolveShortcut(input, context("other", { modalOpen: true }))).toBeNull();
    }
  });

  it("ignores auto-repeat for toggles only", () => {
    expect(resolveShortcut(key({ key: "F9", code: "F9", repeat: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...digit(0), ctrlKey: true, altKey: true, repeat: true }), context("other"))).toBeNull();
    expect(resolveShortcut(key({ ...letter("q"), ctrlKey: true, repeat: true }), context("other"))).toBeNull();
    expect(resolveShortcut({ ...ctrlN, repeat: true }, context("other"))).toEqual({ kind: "new-task" });
  });

  it("never claims an unmodified letter or digit", () => {
    for (const character of "abcdefghijklmnopqrstuvwxyz") {
      expect(resolveShortcut(key(letter(character)), context("other"))).toBeNull();
      expect(resolveShortcut(key({ ...letter(character), shiftKey: true }), context("other"))).toBeNull();
      expect(resolveShortcut(key(letter(character)), context("mac"))).toBeNull();
    }
    for (let value = 0; value <= 9; value += 1) {
      expect(resolveShortcut(key(digit(value)), context("other"))).toBeNull();
      expect(resolveShortcut(key({ ...digit(value), altKey: true }), context("other"))).toBeNull();
    }
    expect(resolveShortcut(key({ key: ",", code: "Comma" }), context("other"))).toBeNull();
  });

  it("fires F9 and F11 only without modifiers", () => {
    for (const name of ["F9", "F11"]) {
      expect(resolveShortcut(key({ key: name, code: name, ctrlKey: true }), context("other"))).toBeNull();
      expect(resolveShortcut(key({ key: name, code: name, shiftKey: true }), context("other"))).toBeNull();
      expect(resolveShortcut(key({ key: name, code: name, altKey: true }), context("other"))).toBeNull();
    }
  });

  it("claims Escape only when something can be dismissed", () => {
    const escape = key({ key: "Escape", code: "Escape" });
    expect(resolveShortcut(escape, context("other", { dismissible: false }))).toBeNull();
    expect(resolveShortcut({ ...escape, shiftKey: true }, context("other"))).toBeNull();
  });

  it("never resolves an intent that has not shipped", () => {
    const enabled = new Set<ShellIntentKind>(["search"]);
    expect(resolveShortcut(key({ key: "F11", code: "F11" }), context("other", { enabled }))).toBeNull();
    expect(resolveShortcut(key({ ...letter("w"), ctrlKey: true }), context("other", { enabled }))).toBeNull();
    expect(resolveShortcut(key({ ...letter("q"), ctrlKey: true }), context("other", { enabled }))).toBeNull();
    expect(resolveShortcut(ctrlN, context("other", { enabled }))).toBeNull();
  });

  it("resolves F11, Ctrl+W and Ctrl+Q in the shipped build, but not held down", () => {
    const enabled = ENABLED_SHORTCUTS;
    expect(resolveShortcut(key({ key: "F11", code: "F11" }), context("other", { enabled }))).toEqual({
      kind: "toggle-fullscreen",
    });
    expect(resolveShortcut(key({ ...letter("w"), ctrlKey: true }), context("other", { enabled }))).toEqual({
      kind: "close-window",
    });
    expect(resolveShortcut(key({ ...letter("q"), ctrlKey: true }), context("other", { enabled }))).toEqual({
      kind: "quit",
    });
    expect(resolveShortcut(key({ key: "F11", code: "F11", repeat: true }), context("other", { enabled }))).toBeNull();
    expect(resolveShortcut(key({ key: "F11", code: "F11", shiftKey: true }), context("other", { enabled }))).toBeNull();
    expect(
      resolveShortcut(key({ ...letter("q"), ctrlKey: true }), context("other", { enabled, modalOpen: true })),
    ).toBeNull();
  });
});

describe("shortcut labels", () => {
  it("writes bindings per platform", () => {
    expect(shortcutLabel("new-task", "other")).toBe("Ctrl+N");
    expect(shortcutLabel("new-task", "mac")).toBe("⌘N");
    expect(shortcutLabel("search", "other")).toBe("Ctrl+K");
    expect(shortcutLabel("add-project", "other")).toBe("Ctrl+Shift+O");
    expect(shortcutLabel("add-project", "mac")).toBe("⇧⌘O");
    expect(shortcutLabel("settings", "other")).toBe("Ctrl+,");
    expect(shortcutLabel("toggle-sidebar", "other")).toBe("F9");
    expect(shortcutLabel("toggle-sidebar", "mac")).toBe("⌃⌘S");
    expect(shortcutLabel("toggle-work-panel", "other")).toBe("Ctrl+Alt+0");
    expect(shortcutLabel("toggle-work-panel", "mac")).toBe("⌥⌘0");
    expect(shortcutLabel("work-panel-tab", "other", "run")).toBe("Ctrl+Alt+4");
    expect(shortcutLabel("dismiss", "other")).toBe("Esc");
    expect(shortcutLabel("toggle-fullscreen", "other")).toBe("F11");
    expect(shortcutLabel("toggle-fullscreen", "mac")).toBe("⌃⌘F");
    expect(shortcutLabel("close-window", "other")).toBe("Ctrl+W");
    expect(shortcutLabel("quit", "other")).toBe("Ctrl+Q");
    expect(shortcutLabel("quit", "mac")).toBe("⌘Q");
  });

  it("writes aria-keyshortcuts in UI Events key names", () => {
    expect(shortcutAria("new-task", "other")).toBe("Control+N");
    expect(shortcutAria("new-task", "mac")).toBe("Meta+N");
    expect(shortcutAria("add-project", "other")).toBe("Control+Shift+O");
    expect(shortcutAria("toggle-sidebar", "other")).toBe("F9");
    expect(shortcutAria("work-panel-tab", "other", "delivery")).toBe("Control+Alt+5");
    expect(shortcutAria("work-panel-tab", "mac", "changes")).toBe("Meta+Alt+1");
  });

  it("every work panel tab has its own digit", () => {
    const labels = WORK_PANEL_TAB_ORDER.map((tab) => shortcutLabel("work-panel-tab", "other", tab));
    expect(labels).toEqual(["Ctrl+Alt+1", "Ctrl+Alt+2", "Ctrl+Alt+3", "Ctrl+Alt+4", "Ctrl+Alt+5"]);
  });
});

describe("detectPlatform", () => {
  it("reads WebKitGTK as Linux and Safari as macOS", () => {
    expect(
      detectPlatform("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15"),
    ).toBe("other");
    expect(
      detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15"),
    ).toBe("mac");
  });
});

describe("SHORTCUTS", () => {
  it("has unique intents, descriptions and bindings", () => {
    const intents = SHORTCUTS.map((shortcut) => shortcut.intent);
    const descriptions = SHORTCUTS.map((shortcut) => shortcut.description);
    expect(new Set(intents).size).toBe(intents.length);
    expect(new Set(descriptions).size).toBe(descriptions.length);
    for (const platform of ["other", "mac"] as const) {
      const bindings = SHORTCUTS.flatMap((shortcut) =>
        shortcut.intent === "work-panel-tab"
          ? WORK_PANEL_TAB_ORDER.map((tab) => shortcutLabel("work-panel-tab", platform, tab))
          : [shortcutLabel(shortcut.intent, platform)],
      );
      expect(new Set(bindings).size).toBe(bindings.length);
    }
  });

  it("lists exactly the shipped intents, so no binding is a no-op", () => {
    for (const shortcut of SHORTCUTS) expect(ENABLED_SHORTCUTS.has(shortcut.intent)).toBe(true);
    expect([...ENABLED_SHORTCUTS].sort()).toEqual(
      [
        "add-project",
        "close-window",
        "dismiss",
        "new-task",
        "quit",
        "search",
        "settings",
        "toggle-fullscreen",
        "toggle-sidebar",
        "toggle-work-panel",
        "work-panel-tab",
      ].sort(),
    );
  });
});
