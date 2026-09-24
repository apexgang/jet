import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { publicError } from "../src/lib/jet/errors";
import {
  DEFAULT_PRESENTATION,
  RESTORABLE_DESTINATIONS,
  closeMainWindow,
  loadShellPresentation,
  presentationErrorCopy,
  quitJet,
  saveShellPresentation,
  toRestorable,
  toggleMainWindowFullscreen,
  type ShellPresentation,
} from "../src/lib/jet/presentation";
import type { SidebarDestination } from "../src/lib/features/shell/session.svelte";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function record(reply: (command: string, args: unknown) => unknown) {
  const calls: Array<{ command: string; args: unknown }> = [];
  vi.stubGlobal("window", {});
  mockIPC((command, args) => {
    calls.push({ command, args });
    return reply(command, args);
  });
  return calls;
}

async function refusal(promise: Promise<unknown>) {
  return promise.then(
    () => {
      throw new Error("expected a refusal");
    },
    (error: unknown) => publicError(error),
  );
}

describe("window layout adapter", () => {
  it("sends the exact command names and argument keys", async () => {
    const saved: ShellPresentation = { ...DEFAULT_PRESENTATION, destination: "planes", sidebarWidth: 260 };
    const calls = record((command, args) => {
      switch (command) {
        case "load_shell_presentation":
          return { presentation: DEFAULT_PRESENTATION, issue: "presentation.read_failed" };
        case "save_shell_presentation":
          return { presentation: (args as { presentation: ShellPresentation }).presentation, issue: null };
        case "toggle_main_window_fullscreen":
          return { fullscreen: true };
        default:
          return null;
      }
    });
    expect(await loadShellPresentation()).toEqual({
      presentation: DEFAULT_PRESENTATION,
      issue: "presentation.read_failed",
    });
    expect(await saveShellPresentation(saved)).toEqual({ presentation: saved, issue: null });
    expect(await toggleMainWindowFullscreen()).toEqual({ fullscreen: true });
    await closeMainWindow();
    await quitJet();
    expect(calls).toEqual([
      { command: "load_shell_presentation", args: {} },
      { command: "save_shell_presentation", args: { presentation: saved } },
      { command: "toggle_main_window_fullscreen", args: {} },
      { command: "close_main_window", args: {} },
      { command: "quit_jet", args: {} },
    ]);
  });

  it("surfaces refusals as PublicErrors with their codes", async () => {
    record((command) => {
      if (command === "save_shell_presentation") {
        throw {
          category: "internal",
          code: "presentation.write_failed",
          message: "Jet couldn't save the window layout.",
          retryable: true,
        };
      }
      throw {
        category: "internal",
        code: "window.mode_unavailable",
        message: "Full screen isn't available in this window.",
        retryable: false,
      };
    });
    const save = await refusal(saveShellPresentation(DEFAULT_PRESENTATION));
    expect(save).toMatchObject({ code: "presentation.write_failed", retryable: true });
    const mode = await refusal(toggleMainWindowFullscreen());
    expect(mode).toMatchObject({ code: "window.mode_unavailable", retryable: false });
  });

  it("refuses a malformed reply instead of trusting it", async () => {
    record(() => ({ presentation: { ...DEFAULT_PRESENTATION, destination: "search" }, issue: null }));
    expect(await refusal(loadShellPresentation())).toMatchObject({ code: "presentation.invalid_response" });
    record(() => ({ presentation: DEFAULT_PRESENTATION, issue: "disk.full" }));
    expect(await refusal(loadShellPresentation())).toMatchObject({ code: "presentation.invalid_response" });
    record(() => null);
    expect(await refusal(loadShellPresentation())).toMatchObject({ code: "presentation.invalid_response" });
  });
});

describe("presentationErrorCopy", () => {
  it("keys on the code, never the message", () => {
    expect(presentationErrorCopy("presentation.read_failed")).toBe(
      "Jet couldn't read the saved window layout, so it started with the default layout.",
    );
    expect(presentationErrorCopy("presentation.write_failed")).toBe(
      "Jet couldn't save the window layout. It'll try again when the layout changes.",
    );
    expect(presentationErrorCopy("presentation.out_of_range")).toBe("Jet couldn't save the window layout.");
    expect(presentationErrorCopy("window.mode_unavailable")).toBe("Full screen isn't available in this window.");
    expect(presentationErrorCopy("window.close_failed")).toBe("Jet couldn't complete that window action.");

    const first = publicError({ code: "window.mode_unavailable", message: "one wording" });
    const second = publicError({ code: "window.mode_unavailable", message: "another wording" });
    expect(presentationErrorCopy(first.code)).toBe(presentationErrorCopy(second.code));
  });
});

describe("toRestorable", () => {
  it("maps every sidebar destination into the restorable set", () => {
    const destinations: Record<SidebarDestination, true> = {
      "new-task": true,
      search: true,
      attention: true,
      project: true,
      conversation: true,
      schedules: true,
      trash: true,
      planes: true,
    };
    for (const destination of Object.keys(destinations) as SidebarDestination[]) {
      expect(RESTORABLE_DESTINATIONS).toContain(toRestorable(destination));
    }
    expect(toRestorable("search")).toBe("conversation");
    expect(toRestorable("attention")).toBe("conversation");
    expect(toRestorable("trash")).toBe("conversation");
    expect(toRestorable("settings")).toBe("conversation");
    expect(toRestorable("new-task")).toBe("new-task");
    expect(toRestorable("planes")).toBe("planes");
  });
});
