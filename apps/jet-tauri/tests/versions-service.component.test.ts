// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { DesktopPreferences, DesktopPreferencesChange } from "../src/lib/jet/preferences";
import type { AppUpdate } from "../src/lib/jet/updates";
import type { Channel } from "@tauri-apps/api/core";
import type { PublicError } from "../src/lib/jet/bridge";
import type { LocalServiceView } from "../src/lib/jet/local-service";
import type { PlaneDetail } from "../src/lib/jet/planes";
import ConnectionsPane from "../src/lib/features/settings/ConnectionsPane.svelte";
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
          return { revision: 0, currentVersion: "0.2.0", state: { kind: "idle", upToDate: false } };
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
    updatesIpc({ revision: 0, currentVersion: "0.2.0", state: { kind: "disabled", reason: "homebrew" } });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    expect(document.body.textContent).toContain("Homebrew keeps this copy of Jet up to date.");
    expect(button("Check for updates")).toBeUndefined();
    expect(document.querySelector('input[type="checkbox"]')).toBeNull();
  });

  it("offers a check and the automatic-check preference with its privacy note", async () => {
    updatesIpc({ revision: 0, currentVersion: "0.2.0", state: { kind: "idle", upToDate: false } });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    expect(button("Check for updates")).toBeDefined();
    const toggle = document.querySelector<HTMLInputElement>('input[type="checkbox"]');
    expect(toggle?.checked).toBe(true);
    expect(toggle?.closest("label")?.textContent).toContain("Check for updates automatically");
    expect(document.body.textContent).toContain("github.com");
  });

  it("asks before restarting into an installed update", async () => {
    updatesIpc({ revision: 0, currentVersion: "0.2.0", state: { kind: "ready", version: "0.3.0" } });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    await fireEvent.click(button("Restart Jet…")!);
    await settle();
    expect(document.body.textContent).toContain("Restart Jet with version 0.3.0?");
    expect(button("Later")).toBeDefined();
  });
});

describe("Settings › Connections: the local service", () => {
  const offline: PublicError = {
    category: "offline",
    code: "transport.offline",
    message: "Jet can't reach this Plane.",
    retryable: true,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: "local",
  };

  /**
   * Renders the pane over a local Plane whose reads follow `jetd`: the core
   * version that answers, or null while no daemon runs, and the store state
   * it reports (serving unless set). `push` sends a view from the native
   * service watcher.
   */
  async function renderPane(jetd: { core: string | null; store?: string }, initial: LocalServiceView) {
    let push: (view: LocalServiceView) => void = () => undefined;
    const calls: string[] = [];
    const plane = () => ({
      planeId: "local",
      kind: "local",
      label: "This computer",
      planeIdentity: null,
      connection: jetd.core === null ? { state: "reconnecting", error: offline } : { state: "online" },
      coreVersion: jetd.core,
      credential: null,
      security: "trusted",
      store: jetd.store ?? "serving",
      features: [],
      protocol: { exact: null, atLeast: 37, atMost: null },
    });
    mockIPC((command, args) => {
      calls.push(command);
      switch (command) {
        case "watch_local_service": {
          const channel = (args as { onChange: Channel<LocalServiceView> }).onChange;
          push = (view) => channel.onmessage(view);
          return initial;
        }
        case "load_plane_detail": {
          const detail: PlaneDetail = {
            plane: plane() as PlaneDetail["plane"],
            platform: "linux",
            harnesses: [],
            crafts: [],
            degraded: [],
            missingTools: [],
            issues: jetd.core === null ? [{ section: "connection", error: offline }] : [],
          };
          return detail;
        }
        case "list_planes":
          return {
            planes: [plane()],
            identity: { clientId: "00000000-0000-4000-8000-00000000000c", key: "unknown", fingerprint: null, notice: null },
            restoredSelection: null,
            notice: null,
            maximumRemotePlanes: 16,
          };
        default:
          return null;
      }
    });
    const service = new LocalServiceSession();
    render(ConnectionsPane, { service });
    await settle();
    expect(calls).toContain("watch_local_service");
    return {
      push: async (view: LocalServiceView) => {
        push(view);
        await settle();
      },
      reads: () => calls.filter((command) => command === "load_plane_detail").length,
    };
  }

  const fact = (term: string) =>
    [...document.querySelectorAll("dt")].find((candidate) => candidate.textContent === term)?.nextElementSibling
      ?.textContent;

  it("shows the core the local service activated without a refresh", async () => {
    const jetd = { core: "0.2.0" as string | null };
    const pane = await renderPane(jetd, serviceView());
    expect(fact("Version")).toBe("0.2.0");
    const before = pane.reads();

    // Draining for the activation reports the same running version.
    await pane.push(serviceView({ phase: "updating" }));
    expect(pane.reads()).toBe(before);

    // The service manager started the new core.
    jetd.core = "0.3.0";
    await pane.push(serviceView({ currentVersion: "0.3.0", runningVersion: "0.3.0", lastAction: "updated" }));
    expect(pane.reads()).toBe(before + 1);
    expect(fact("Version")).toBe("0.3.0");
  });

  it("shows a service that starts, or stops, after the pane opened", async () => {
    const jetd = { core: null as string | null };
    const pane = await renderPane(jetd, serviceView({ phase: "stopped", runningVersion: null, canRepair: true }));
    expect(fact("Jet service")).toBe("Not reachable");
    const before = pane.reads();

    // Starting, nothing answers yet: there is nothing new to read.
    await pane.push(serviceView({ phase: "starting", runningVersion: null }));
    expect(pane.reads()).toBe(before);

    jetd.core = "0.2.0";
    await pane.push(serviceView({ lastAction: "started" }));
    expect(pane.reads()).toBe(before + 1);
    expect(fact("Jet service")).toBe("Running on this computer");
    expect(fact("Version")).toBe("0.2.0");

    // The daemon stops again: the pane no longer says it runs.
    jetd.core = null;
    await pane.push(serviceView({ phase: "stopped", runningVersion: null, canRepair: true }));
    expect(pane.reads()).toBe(before + 2);
    expect(fact("Jet service")).toBe("Not reachable");
  });

  it("reads the Plane again when a pass ends with the same core running again", async () => {
    const jetd: { core: string | null; store?: string } = { core: "0.2.0" };
    const pane = await renderPane(jetd, serviceView());
    expect(fact("Storage")).toBe("Serving");
    const before = pane.reads();

    await pane.push(serviceView({ phase: "updating" }));
    expect(pane.reads()).toBe(before);

    // The activation failed after the old core drained; the service manager
    // started that core again, and its new run is in recovery.
    jetd.store = "read_only";
    const failed: PublicError = {
      ...offline,
      category: "local_unavailable",
      code: "service.install_failed",
      message: "The Jet service couldn't be set up on this computer.",
      planeId: null,
    };
    await pane.push(serviceView({ error: failed }));
    expect(pane.reads()).toBe(before + 1);
    expect(fact("Version")).toBe("0.2.0");
    expect(fact("Storage")).toBe("Read-only recovery");
  });
});

describe("Settings › Versions: focus and announcements (finding 13)", () => {
  const liveRegion = (block: string) =>
    document.querySelector<HTMLElement>(`.${block} [role="status"][aria-live="polite"]`)?.textContent?.trim();
  const focused = () => (document.activeElement as HTMLElement | null)?.textContent?.trim() ?? null;

  /** The shell as the webview sees it: `push` publishes the next state to the watcher. */
  function updatesShell(initial: AppUpdate["state"], onCommand: (command: string) => AppUpdate["state"] | null = () => null) {
    let watcher: Channel<AppUpdate> | null = null;
    let revision = 1;
    let state = initial;
    const view = (): AppUpdate => ({ revision, currentVersion: "0.2.0", state });
    mockIPC((command, args) => {
      switch (command) {
        case "watch_app_update":
          watcher = (args as { onChange: Channel<AppUpdate> }).onChange;
          return view();
        case "load_desktop_preferences":
          return { reopenLastTask: true, checkForUpdates: true };
        default: {
          const next = onCommand(command);
          if (next) {
            state = next;
            revision += 1;
          }
          return view();
        }
      }
    });
    return {
      push: async (next: AppUpdate["state"]) => {
        state = next;
        revision += 1;
        watcher!.onmessage(view());
        await settle();
      },
    };
  }

  it("announces each update step once, not every percent of the download", async () => {
    const shell = updatesShell({ kind: "downloading", version: "0.3.0", downloaded: 10, total: 100 });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    const spoken = liveRegion("app-updates");
    expect(spoken).toBe("Downloading Jet 0.3.0…");
    await shell.push({ kind: "downloading", version: "0.3.0", downloaded: 60, total: 100 });
    expect(liveRegion("app-updates")).toBe(spoken);
    // The progress stays visible outside the live region.
    expect(document.querySelector("progress")?.getAttribute("value")).toBe("60");
    expect(document.body.textContent).toContain("60%");
    await shell.push({ kind: "ready", version: "0.3.0" });
    expect(liveRegion("app-updates")).toBe("Jet 0.3.0 is installed. Restart Jet to use it.");
  });

  it("keeps focus on the update action while it moves from Install to Restart", async () => {
    const shell = updatesShell({ kind: "available", version: "0.3.0", dateUnixMs: null }, (command) =>
      command === "install_app_update" ? { kind: "downloading", version: "0.3.0", downloaded: 0, total: 100 } : null,
    );
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    const install = button("Install Jet 0.3.0")!;
    install.focus();
    await fireEvent.click(install);
    await settle();
    expect(focused()).toBe("Installing…");
    await shell.push({ kind: "ready", version: "0.3.0" });
    expect(focused()).toBe("Restart Jet…");
  });

  it("moves focus to the heading when the update controls go away", async () => {
    const shell = updatesShell({ kind: "idle", upToDate: false });
    render(AppUpdatesBlock, { updates: new AppUpdateSession() });
    await settle();
    button("Check for updates")!.focus();
    await shell.push({ kind: "disabled", reason: "homebrew" });
    expect(document.activeElement?.id).toBe("app-updates-heading");
  });

  it("announces a failed service pass and keeps focus through a Repair", async () => {
    let push: (view: LocalServiceView) => void = () => undefined;
    let finish: () => void = () => undefined;
    const failed: PublicError = {
      category: "local_unavailable",
      code: "service.start_timeout",
      message: "The Jet service was started but didn't answer in time.",
      retryable: true,
      recoveryActions: [],
      restart: null,
      revisionConflict: null,
      protocolLimit: null,
      planeId: null,
    };
    mockIPC(async (command, args) => {
      switch (command) {
        case "watch_local_service":
          push = (args as { onChange: Channel<LocalServiceView> }).onChange.onmessage;
          return serviceView({ revision: 1, phase: "failed", runningVersion: null, canRepair: true, error: failed });
        case "repair_local_service":
          push(serviceView({ revision: 2, phase: "checking", runningVersion: null }));
          await new Promise<void>((resolve) => (finish = resolve));
          push(serviceView({ revision: 3, lastAction: "started" }));
          return serviceView({ revision: 3, lastAction: "started" });
        default:
          return null;
      }
    });
    render(LocalServiceBlock, { service: new LocalServiceSession() });
    await settle();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("didn't answer in time");

    const repair = button("Repair")!;
    repair.focus();
    await fireEvent.click(repair);
    await settle();
    // The pass runs: the same button stays, busy, and keeps focus.
    expect(focused()).toBe("Repairing…");
    expect(document.activeElement?.getAttribute("aria-disabled")).toBe("true");
    finish();
    await settle();
    // Running again: Repair is gone, and focus lands on the block's heading.
    expect(button("Repair")).toBeUndefined();
    expect(document.activeElement?.id).toBe("local-service-heading");
  });
});
