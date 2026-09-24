// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { DesktopPreferences, DesktopPreferencesChange } from "../src/lib/jet/preferences";
import type { AppUpdate } from "../src/lib/jet/updates";
import GeneralPane from "../src/lib/features/settings/GeneralPane.svelte";
import AppUpdatesBlock from "../src/lib/features/system/AppUpdatesBlock.svelte";
import LocalServiceBlock from "../src/lib/features/system/LocalServiceBlock.svelte";
import { LocalServiceSession } from "../src/lib/features/system/local-service.svelte";
import { AppUpdateSession } from "../src/lib/features/system/updates.svelte";
import { serviceView } from "./support/service";

afterEach(() => {
  cleanup();
  clearMocks();
});

async function settle() {
  for (let round = 0; round < 5; round += 1) {
    flushSync();
    await tick();
    for (let micro = 0; micro < 10; micro += 1) await Promise.resolve();
  }
}

const button = (name: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((candidate) => candidate.textContent?.trim() === name);

const checkbox = (label: string) =>
  [...document.querySelectorAll<HTMLLabelElement>("label")]
    .find((candidate) => candidate.textContent?.includes(label))
    ?.querySelector<HTMLInputElement>('input[type="checkbox"]');

async function toggle(label: string, checked: boolean) {
  const input = checkbox(label);
  expect(input, label).toBeTruthy();
  input!.checked = checked;
  await fireEvent.change(input!);
  await settle();
}

describe("Settings › Versions: the Jet service on this computer", () => {
  it("names the channel and versions and reviews a rollback before sending it", async () => {
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      switch (command) {
        case "watch_local_service":
          return serviceView({ previousVersion: "0.1.0", canRollback: true });
        case "prepare_local_service_rollback":
          return { reviewId: "r1", currentVersion: "0.2.0", previousVersion: "0.1.0" };
        case "execute_local_service_rollback":
          return serviceView({ currentVersion: "0.1.0", previousVersion: "0.2.0", lastAction: "rolled_back" });
        default:
          return null;
      }
    });
    const service = new LocalServiceSession();
    render(LocalServiceBlock, { service });
    await settle();
    const text = document.body.textContent ?? "";
    expect(text).toContain("Managed by this app");
    expect(text).toContain("systemd user service");
    expect(text).toContain("Previous version");
    expect(button("Repair")).toBeUndefined();

    await fireEvent.click(button("Roll back to 0.1.0…")!);
    await settle();
    expect(calls).toContain("prepare_local_service_rollback");
    expect(calls).not.toContain("execute_local_service_rollback");
    expect(document.body.textContent).toContain("Go back to Jet service 0.1.0?");

    await fireEvent.click(button("Roll back")!);
    await settle();
    expect(calls.filter((command) => command === "execute_local_service_rollback")).toHaveLength(1);
    expect(document.body.textContent).toContain("The Jet service went back to 0.1.0.");
  });

  it("says Homebrew manages it and offers no rollback", async () => {
    mockIPC((command) =>
      command === "watch_local_service"
        ? serviceView({ channel: "homebrew", manager: "brew_services", previousVersion: null })
        : null,
    );
    const service = new LocalServiceSession();
    render(LocalServiceBlock, { service });
    await settle();
    expect(document.body.textContent).toContain("Managed by Homebrew");
    expect(document.body.textContent).not.toContain("Included with this app");
    expect([...document.querySelectorAll("button")].some((node) => node.textContent?.includes("Roll back"))).toBe(false);
  });
});

describe("Settings › Versions: App updates", () => {
  function updatesIpc(update: AppUpdate) {
    mockIPC((command) => {
      if (command === "watch_app_update") return update;
      if (command === "load_desktop_preferences") return { reopenLastTask: true, checkForUpdates: true };
      return null;
    });
  }

  /**
   * Safety › Versions, then General, then Versions again in one window: the
   * update session keeps the preferences it read first, and each pane
   * saves only its own choice.
   */
  it("keeps General's Reopen choice when Versions saves the automatic check", async () => {
    let stored: DesktopPreferences = { reopenLastTask: true, checkForUpdates: true };
    const saves: DesktopPreferencesChange[] = [];
    mockIPC((command, args) => {
      switch (command) {
        case "watch_app_update":
          return { currentVersion: "0.2.0", state: { kind: "idle", upToDate: false } };
        case "load_desktop_preferences":
          return stored;
        case "set_desktop_preferences": {
          const change = (args as { preferences: DesktopPreferencesChange }).preferences;
          saves.push(change);
          // The shell keeps a preference the change leaves out.
          stored = { ...stored, ...change };
          return stored;
        }
        default:
          throw new Error(`Not needed here: ${command}`);
      }
    });
    const updates = new AppUpdateSession();
    const versions = render(AppUpdatesBlock, { updates });
    await settle();
    expect(checkbox("Check for updates automatically")?.checked).toBe(true);
    versions.unmount();

    const general = render(GeneralPane);
    await settle();
    await toggle("Reopen the last task when Jet starts", false);
    general.unmount();

    render(AppUpdatesBlock, { updates });
    await settle();
    await toggle("Check for updates automatically", false);

    expect(saves).toEqual([{ reopenLastTask: false }, { checkForUpdates: false }]);
    expect(stored).toEqual({ reopenLastTask: false, checkForUpdates: false });
    expect(document.body.textContent).toContain("Saved on this computer.");
  });

  it("explains a disabled updater and hides its controls", async () => {
    updatesIpc({ currentVersion: "0.2.0", state: { kind: "disabled", reason: "homebrew" } });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    expect(document.body.textContent).toContain("Homebrew keeps this copy of Jet up to date.");
    expect(button("Check for updates")).toBeUndefined();
    expect(document.querySelector('input[type="checkbox"]')).toBeNull();
  });

  it("offers a check and the automatic-check preference with its privacy note", async () => {
    updatesIpc({ currentVersion: "0.2.0", state: { kind: "idle", upToDate: false } });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    expect(button("Check for updates")).toBeDefined();
    const toggle = document.querySelector<HTMLInputElement>('input[type="checkbox"]');
    expect(toggle?.checked).toBe(true);
    expect(toggle?.closest("label")?.textContent).toContain("Check for updates automatically");
    expect(document.body.textContent).toContain("github.com");
  });

  it("asks before restarting into an installed update", async () => {
    updatesIpc({ currentVersion: "0.2.0", state: { kind: "ready", version: "0.3.0" } });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    await fireEvent.click(button("Restart Jet…")!);
    await settle();
    expect(document.body.textContent).toContain("Restart Jet with version 0.3.0?");
    expect(button("Later")).toBeDefined();
  });
});
