import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type { PublicError } from "../src/lib/jet/bridge";
import {
  setDesktopPreferences,
  type DesktopPreferences,
  type DesktopPreferencesChange,
} from "../src/lib/jet/preferences";
import {
  checkAppUpdate,
  installAppUpdate,
  loadAppUpdate,
  restartAfterUpdate,
  watchAppUpdate,
  type AppUpdate,
  type AppUpdateState,
} from "../src/lib/jet/updates";
import { AppUpdateSession } from "../src/lib/features/system/updates.svelte";
import {
  downloadPercent,
  updateDisabledText,
  updateErrorText,
  updateStatusText,
} from "../src/lib/features/system/service-model";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function failure(code: string): PublicError {
  return {
    category: "unavailable",
    code,
    message: `${code} message`,
    retryable: true,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

const update = (state: AppUpdateState, revision = 0): AppUpdate => ({ revision, currentVersion: "0.2.0", state });

type Script = {
  state: AppUpdateState;
  check?: AppUpdateState | PublicError;
  install?: { progress: AppUpdateState[]; result: AppUpdateState };
  preferences?: DesktopPreferences;
  restartFails?: boolean;
};

async function flush() {
  for (let tick = 0; tick < 20; tick += 1) await Promise.resolve();
}

/** The shell as the webview sees it: install progress reaches the window's watcher. */
function ipc(script: Script) {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  let preferences = script.preferences ?? { reopenLastTask: false, checkForUpdates: true };
  let watcher: Channel<AppUpdate> | null = null;
  // The shell's revision: every published state gets the next one.
  let revision = 1;
  const publish = (state: AppUpdateState) => {
    script.state = state;
    revision += 1;
    watcher?.onmessage(update(state, revision));
  };
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const { onChange, ...plain } = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: plain });
    switch (command) {
      case "load_app_update":
        return update(script.state, revision);
      case "watch_app_update":
        watcher = onChange as Channel<AppUpdate>;
        return update(script.state, revision);
      case "check_app_update":
        if (script.check && "code" in script.check) throw script.check;
        // As the shell does: `checking`, then the answer, both through the watcher.
        publish({ kind: "checking" });
        await Promise.resolve();
        publish(script.check ?? script.state);
        return update(script.state, revision);
      case "install_app_update": {
        for (const state of script.install?.progress ?? []) publish(state);
        publish(script.install?.result ?? script.state);
        return update(script.state, revision);
      }
      case "restart_after_update":
        if (script.restartFails) throw failure("update.not_ready");
        return null;
      case "load_desktop_preferences":
        return preferences;
      case "set_desktop_preferences":
        // The shell keeps a preference the change leaves out.
        preferences = { ...preferences, ...(plain.preferences as DesktopPreferencesChange) };
        return preferences;
      default:
        throw new Error(`Unexpected ${command}`);
    }
  });
  return {
    calls,
    preferences: () => preferences,
    /** A state the shell reached without this window asking. */
    push: publish,
  };
}

describe("app update adapter", () => {
  it("uses the native commands and follows install progress through the watcher", async () => {
    const { calls } = ipc({
      state: { kind: "idle", upToDate: false },
      check: { kind: "available", version: "0.3.0", dateUnixMs: null },
      install: {
        progress: [{ kind: "downloading", version: "0.3.0", downloaded: 10, total: 100 }],
        result: { kind: "ready", version: "0.3.0" },
      },
    });
    expect((await loadAppUpdate()).state.kind).toBe("idle");
    const seen: AppUpdateState[] = [];
    expect((await watchAppUpdate((next) => seen.push(next.state))).state.kind).toBe("idle");
    expect((await checkAppUpdate()).state.kind).toBe("available");
    expect((await installAppUpdate()).state).toEqual({ kind: "ready", version: "0.3.0" });
    expect(seen).toEqual([
      { kind: "checking" },
      { kind: "available", version: "0.3.0", dateUnixMs: null },
      { kind: "downloading", version: "0.3.0", downloaded: 10, total: 100 },
      { kind: "ready", version: "0.3.0" },
    ]);
    await restartAfterUpdate();
    expect(calls).toEqual([
      { command: "load_app_update", args: {} },
      { command: "watch_app_update", args: {} },
      { command: "check_app_update", args: {} },
      { command: "install_app_update", args: {} },
      { command: "restart_after_update", args: {} },
    ]);
  });
});

describe("AppUpdateSession", () => {
  it("checks, installs with progress, and restarts only after confirmation", async () => {
    const { calls } = ipc({
      state: { kind: "idle", upToDate: false },
      check: { kind: "available", version: "0.3.0", dateUnixMs: "1790000000000" },
      install: {
        progress: [
          { kind: "downloading", version: "0.3.0", downloaded: 0, total: null },
          { kind: "downloading", version: "0.3.0", downloaded: 50, total: 100 },
        ],
        result: { kind: "ready", version: "0.3.0" },
      },
    });
    const session = new AppUpdateSession();
    await session.start();
    expect(session.update?.state).toEqual({ kind: "idle", upToDate: false });
    expect(session.automatic).toMatchObject({ kind: "ready", preferences: { checkForUpdates: true } });

    const checking = session.check();
    expect(session.update?.state.kind).toBe("checking");
    await checking;
    expect(session.update?.state.kind).toBe("available");

    const installing = session.install();
    expect(session.busy).toBe("installing");
    await installing;
    expect(session.update?.state).toEqual({ kind: "ready", version: "0.3.0" });

    // Restarting needs the confirmation first.
    await session.restart();
    expect(calls.some((call) => call.command === "restart_after_update")).toBe(false);
    session.requestRestart();
    expect(session.confirmingRestart).toBe(true);
    session.cancelRestart();
    session.requestRestart();
    await session.restart();
    expect(calls.filter((call) => call.command === "restart_after_update")).toHaveLength(1);
  });

  it("does not install without an announced update and keeps failures readable", async () => {
    const { calls } = ipc({ state: { kind: "idle", upToDate: true }, check: failure("update.offline") });
    const session = new AppUpdateSession();
    await session.start();
    await session.install();
    expect(calls.some((call) => call.command === "install_app_update")).toBe(false);
    await session.check();
    expect(session.error?.code).toBe("update.offline");
    expect(session.busy).toBeNull();
  });

  it("saves only the automatic check, so an older copy can't undo another pane's choice", async () => {
    const script = ipc({
      state: { kind: "idle", upToDate: false },
      preferences: { reopenLastTask: true, checkForUpdates: true },
    });
    const session = new AppUpdateSession();
    await session.start();
    // General turns off "Reopen the last task" after this session read both.
    await setDesktopPreferences({ reopenLastTask: false });
    await session.setAutomatic(false);
    expect(script.calls.filter((call) => call.command === "set_desktop_preferences").map((call) => call.args)).toEqual([
      { preferences: { reopenLastTask: false } },
      { preferences: { checkForUpdates: false } },
    ]);
    expect(script.preferences()).toEqual({ reopenLastTask: false, checkForUpdates: false });
    expect(session.automatic).toMatchObject({
      kind: "ready",
      preferences: { checkForUpdates: false },
      notice: "Saved on this computer.",
    });
  });

  it("follows checks and installs this window did not start", async () => {
    const { push, calls } = ipc({ state: { kind: "downloading", version: "0.3.0", downloaded: 10, total: 100 } });
    const session = new AppUpdateSession();
    // Reopened during an install another window started.
    await session.start();
    expect(session.update?.state.kind).toBe("downloading");
    push({ kind: "downloading", version: "0.3.0", downloaded: 60, total: 100 });
    expect(session.update?.state).toMatchObject({ downloaded: 60 });
    push({ kind: "ready", version: "0.3.0" });
    expect(session.update?.state).toEqual({ kind: "ready", version: "0.3.0" });
    session.requestRestart();
    expect(session.confirmingRestart).toBe(true);
    // One watcher per window.
    await session.start();
    expect(calls.filter((call) => call.command === "watch_app_update")).toHaveLength(1);
    session.dispose();
    push({ kind: "idle", upToDate: true });
    expect(session.update?.state.kind).toBe("ready");
  });

  it("never replaces a pushed state with the older initial answer", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (release = resolve));
    let watcher: Channel<AppUpdate> | null = null;
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC(async (command, args) => {
      if (command === "watch_app_update") {
        watcher = (args as { onChange: Channel<AppUpdate> }).onChange;
        await held;
        return update({ kind: "idle", upToDate: false }, 1);
      }
      if (command === "load_desktop_preferences") return { reopenLastTask: true, checkForUpdates: true };
      throw new Error(command);
    });
    const session = new AppUpdateSession();
    const started = session.start();
    await flush();
    watcher!.onmessage(update({ kind: "checking" }, 2));
    release();
    await started;
    expect(session.update?.state.kind).toBe("checking");
  });

  it("drops a Check reply older than a state the watcher already pushed", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (release = resolve));
    let watcher: Channel<AppUpdate> | null = null;
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC(async (command, args) => {
      switch (command) {
        case "watch_app_update":
          watcher = (args as { onChange: Channel<AppUpdate> }).onChange;
          return update({ kind: "idle", upToDate: false }, 1);
        case "check_app_update":
          await held;
          return update({ kind: "available", version: "0.3.0", dateUnixMs: null }, 3);
        case "load_desktop_preferences":
          return { reopenLastTask: true, checkForUpdates: true };
        default:
          throw new Error(command);
      }
    });
    const session = new AppUpdateSession();
    await session.start();
    const checking = session.check();
    await flush();
    watcher!.onmessage(update({ kind: "available", version: "0.3.0", dateUnixMs: null }, 3));
    // Homebrew took over the service after the check ended.
    watcher!.onmessage(update({ kind: "disabled", reason: "homebrew" }, 4));
    release();
    await checking;
    expect(session.update).toMatchObject({ revision: 4, state: { kind: "disabled", reason: "homebrew" } });
  });

  it("closes the confirmation when the restart is refused", async () => {
    ipc({ state: { kind: "ready", version: "0.3.0" }, restartFails: true });
    const session = new AppUpdateSession();
    await session.start();
    session.requestRestart();
    await session.restart();
    expect(session.confirmingRestart).toBe(false);
    expect(session.error?.code).toBe("update.not_ready");
  });
});

describe("app update copy", () => {
  it("explains why updates are off", () => {
    expect(updateDisabledText("homebrew")).toMatch(/Homebrew/);
    expect(updateDisabledText("development_build")).toBe("This is a development build, so it doesn't update itself.");
    expect(updateDisabledText("unsupported_install")).toMatch(/\.deb, \.rpm or AppImage/);
    expect(updateDisabledText("service_unknown")).toMatch(/still checking who manages it/);
  });

  it("describes every state", () => {
    expect(updateStatusText(update({ kind: "idle", upToDate: true }))).toBe("Jet 0.2.0 is up to date.");
    expect(updateStatusText(update({ kind: "available", version: "0.3.0", dateUnixMs: null }))).toBe(
      "Jet 0.3.0 is available. You have 0.2.0.",
    );
    // Progress is not part of the spoken line.
    expect(updateStatusText(update({ kind: "downloading", version: "0.3.0", downloaded: 25, total: 100 }))).toBe(
      "Downloading Jet 0.3.0…",
    );
    expect(updateStatusText(update({ kind: "downloading", version: "0.3.0", downloaded: 25, total: null }))).toBe(
      "Downloading Jet 0.3.0…",
    );
    expect(updateStatusText(update({ kind: "ready", version: "0.3.0" }))).toBe(
      "Jet 0.3.0 is installed. Restart Jet to use it.",
    );
    expect(updateStatusText(update({ kind: "failed", error: failure("update.signature_invalid") }))).toMatch(
      /isn't signed by Jet/,
    );
    expect(updateErrorText(failure("update.release_unavailable"))).toMatch(/Try again later/);
    expect(updateErrorText(failure("update.package_install_failed"))).toMatch(/password prompt was canceled/);
    expect(downloadPercent(150, 100)).toBe(100);
    expect(downloadPercent(1, 0)).toBeNull();
  });
});
