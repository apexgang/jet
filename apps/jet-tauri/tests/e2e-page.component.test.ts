// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { PublicError, SetupSnapshot } from "../src/lib/jet/bridge";
import type { PlanesSnapshot } from "../src/lib/jet/planes";
import AppShell from "../src/lib/features/shell/AppShell.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import SettingsApp from "../src/lib/features/settings/SettingsApp.svelte";
import { updaterOn } from "./e2e/journey";
import { PAGE_SCRIPT, type ControlQuery, type MainView, type SettingsView } from "./e2e/page";
import { serviceView } from "./support/service";

/**
 * The release journey reads the app through `PAGE_SCRIPT`, sent as a
 * WebDriver script. These tests run that exact source against the real
 * components, so a renamed heading, label or landmark fails here rather
 * than in a CI-only journey.
 */
function page<T>(command: string, argument?: unknown): T {
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  return new Function(PAGE_SCRIPT)(command, argument) as T;
}

const SIDEBAR: ControlQuery["scope"] = { role: "complementary", name: "Jet navigation" };

function offline(): PublicError {
  return {
    category: "offline",
    code: "transport.offline",
    message: "Jet could not reach this Plane.",
    retryable: true,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: "local",
  };
}

function setup(projects: SetupSnapshot["projects"] = [], coreVersion = "0.2.0", daemonStarts = "1"): SetupSnapshot {
  return {
    plane: { coreVersion, daemonStarts, platform: "linux" },
    capabilities: {
      harnesses: ["codex"],
      crafts: [],
      credentialStore: "available",
      credentialStoreLabel: "Keyring",
      degraded: [],
      authProviders: [],
    },
    projects,
    accounts: [],
    pairing: { gate: "closed", pairedClients: 0, offerPending: false },
    issues: [],
  };
}

const PLANES = {
  planes: [
    {
      planeId: "local",
      kind: "local",
      label: "This computer",
      planeIdentity: null,
      connection: { state: "online" },
      coreVersion: "0.2.0",
      credential: null,
      security: "trusted",
      store: "serving",
      features: [],
      protocol: { exact: null, atLeast: 37, atMost: null },
    },
  ],
  identity: { clientId: "00000000-0000-4000-8000-00000000000c", key: "unknown", fingerprint: null, notice: null },
  restoredSelection: null,
  notice: null,
  maximumRemotePlanes: 16,
} as unknown as PlanesSnapshot;

async function settle() {
  for (let round = 0; round < 5; round += 1) {
    flushSync();
    await tick();
    for (let micro = 0; micro < 10; micro += 1) await Promise.resolve();
  }
}

beforeEach(() => performance.clearMarks());
afterEach(() => {
  cleanup();
  clearMocks();
});

describe("the journey's page script in the main window", () => {
  it("reads the navigation landmark, its Plane status and the composer", async () => {
    const session = new DesktopSession();
    render(AppShell, { session });
    await settle();
    const view = page<MainView>("main");
    expect(view.planeStatus).toMatch(/Connecting$/);
    expect(view.planeState).toBe("Connecting");
    expect(view.composer).toBe(true);
    expect(view.setup).toBeNull();
    expect(typeof view.timeOrigin).toBe("number");

    session.connectionState = "online";
    await settle();
    expect(page<MainView>("main").planeState).toBe("Connected");
  });

  it("reads the aggregate Plane status once the registry has loaded", async () => {
    mockIPC((command) => (command === "list_planes" ? PLANES : null));
    const session = new DesktopSession();
    await session.planes.refresh();
    render(AppShell, { session });
    await settle();
    const view = page<MainView>("main");
    expect(view.planeStatus).toBe("This computer Connected");
    expect(view.planeState).toBe("Connected");
  });

  it("reads Connected and the restarted core once the local feed dials jetd again", async () => {
    // The journey's reconnect step waits for exactly this after
    // `systemctl --user kill jetd.service`.
    let connection: { state: string; error?: PublicError } = { state: "online" };
    let core = { version: "0.2.0", starts: "1" };
    mockIPC((command) => {
      switch (command) {
        case "open_plane_feed":
          return {
            state: "online",
            feedId: "feed-1",
            planeId: "local",
            planeIdentity: null,
            health: { security: "trusted", store: "serving" },
            coreVersion: "0.2.0",
            daemonStarts: "1",
            startedAtUnixMs: "1",
            cursor: "40",
          };
        case "list_planes":
          return { ...PLANES, planes: [{ ...PLANES.planes[0], connection, coreVersion: core.version }] };
        case "load_setup":
          return setup([], core.version, core.starts);
        case "load_conversations":
          return { planeId: "local", cursor: "40", conversations: [], nextPage: null };
        default:
          return null;
      }
    });
    const session = new DesktopSession();
    session.sidebarSelection = "project";
    session.service.view = serviceView();
    // The feed is open, as it is in the app: the Recent reload that follows
    // `resumed` opens no other one.
    await session.planes.openFeed("local", null);
    await session.refreshSetup();
    render(AppShell, { session });
    await settle();
    expect(page<MainView>("main").setup?.localPlane?.coreVersion).toBe("0.2.0");
    const before = performance.now();

    connection = { state: "reconnecting", error: offline() };
    session.receive("local", { type: "reconnecting", error: offline() });
    await settle();
    expect(page<MainView>("main").planeStatus).toBe("This computer Reconnecting");

    // The feed dials the restarted jetd, reads its status (the native
    // registry records it), says connected, then resumed.
    connection = { state: "online" };
    core = { version: "0.3.0", starts: "2" };
    session.receive("local", {
      type: "connected",
      connection: {
        state: "online",
        feedId: "feed-1",
        planeId: "local",
        planeIdentity: null,
        health: { security: "trusted", store: "serving", ledger: "verified" },
        coreVersion: "0.3.0",
        daemonStarts: "2",
        startedAtUnixMs: "2",
        cursor: "40",
      },
    });
    session.receive("local", { type: "resumed", after: "40" });
    await settle();
    const view = page<MainView>("main");
    expect(view.planeStatus).toBe("This computer Connected");
    expect(view.planeState).toBe("Connected");
    expect(view.setup?.localPlane?.coreVersion).toBe("0.3.0");
    expect(view.marks.filter((mark) => mark.name === "local-plane-connected" && mark.startTime >= before)).toHaveLength(1);
  });

  it("reports the shell-interactive mark after the first frame", async () => {
    render(AppShell, { session: new DesktopSession() });
    await settle();
    await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
    await settle();
    const view = page<MainView>("main");
    expect(view.marks.map((mark) => mark.name)).toContain("shell-interactive");
  });

  it("finds the sidebar's Setup and Settings buttons by accessible name", async () => {
    const session = new DesktopSession();
    render(AppShell, { session });
    await settle();
    const setupButton = page<HTMLElement | null>("button", { scope: SIDEBAR, names: ["Add a Project", "Manage Projects"] });
    expect(setupButton?.tagName).toBe("BUTTON");
    expect(setupButton?.textContent?.trim()).toBe("Add a Project");
    const settings = page<HTMLElement | null>("button", { scope: SIDEBAR, names: ["Settings"] });
    expect(settings?.textContent).toContain("Settings");
    expect(page("button", { scope: SIDEBAR, names: ["No such button"] })).toBeNull();
    expect(page("button", { scope: { role: "complementary", name: "Elsewhere" }, names: ["Settings"] })).toBeNull();

    await fireEvent.click(setupButton!);
    await settle();
    expect(session.sidebarSelection).toBe("project");
    expect(page<MainView>("main").setup).not.toBeNull();
  });

  it("offers Manage Projects once a Project exists", async () => {
    const session = new DesktopSession();
    session.setup = { kind: "ready", snapshot: setup([{ id: "p1", name: "Jet", root: "/w" }]) };
    render(AppShell, { session });
    await settle();
    const button = page<HTMLElement | null>("button", { scope: SIDEBAR, names: ["Add a Project", "Manage Projects"] });
    expect(button?.textContent?.trim()).toBe("Manage Projects");
  });

  it("follows Setup from provisioning to the connected local Plane", async () => {
    const session = new DesktopSession();
    session.sidebarSelection = "project";
    session.setup = { kind: "failed", error: offline() };
    render(AppShell, { session });
    await settle();
    // Before the service watcher answers, Setup shows the generic failure.
    expect(page<MainView>("main").setup?.failure?.title).toBe("Local Plane unavailable");

    session.service.view = serviceView({ phase: "checking", currentVersion: null, runningVersion: null });
    await settle();
    expect(page<MainView>("main").setup?.provisioning).toBe("Checking the Jet service on this computer…");

    session.service.view = serviceView({ phase: "installing", currentVersion: null, runningVersion: null });
    await settle();
    let view = page<MainView>("main").setup!;
    expect(view.provisioning).toBe("Setting up the Jet service on this computer…");
    expect(view.failure).toBeNull();
    expect(view.localPlane).toBeNull();

    session.service.view = serviceView({ phase: "starting", currentVersion: "0.2.0", runningVersion: null });
    await settle();
    expect(page<MainView>("main").setup?.provisioning).toBe("Starting the Jet service…");

    session.service.view = serviceView({ lastAction: "installed" });
    session.setup = { kind: "ready", snapshot: setup() };
    await settle();
    view = page<MainView>("main").setup!;
    expect(view.provisioning).toBeNull();
    expect(view.failure).toBeNull();
    expect(view.localPlane).toEqual({
      text: expect.stringContaining("Core 0.2.0"),
      coreVersion: "0.2.0",
      status: "Connected",
      notice: "The Jet service 0.2.0 is set up on this computer.",
    });
  });

  it("reads a relaunch's Local Plane without a setup notice", async () => {
    const session = new DesktopSession();
    session.sidebarSelection = "project";
    session.service.view = serviceView();
    session.setup = { kind: "ready", snapshot: setup() };
    render(AppShell, { session });
    await settle();
    const plane = page<MainView>("main").setup!.localPlane!;
    expect(plane.coreVersion).toBe("0.2.0");
    expect(plane.notice).toBeNull();
    expect(plane.status).toBe("Connected");
  });

  it("reads a Local Plane that needs attention as connected but degraded", async () => {
    const session = new DesktopSession();
    session.sidebarSelection = "project";
    const degraded = setup();
    degraded.capabilities.degraded = ["No credential store"];
    session.setup = { kind: "ready", snapshot: degraded };
    render(AppShell, { session });
    await settle();
    expect(page<MainView>("main").setup!.localPlane!.status).toBe("Needs attention");
  });

  it("reads a settled service failure with its code", async () => {
    const session = new DesktopSession();
    session.sidebarSelection = "project";
    session.setup = { kind: "failed", error: offline() };
    session.service.view = serviceView({
      phase: "failed",
      canRepair: true,
      error: { ...offline(), category: "unavailable", code: "service.install_failed", planeId: null },
    });
    render(AppShell, { session });
    await settle();
    const failure = page<MainView>("main").setup!.failure!;
    expect(failure.title).toBe("The Jet service needs attention");
    expect(failure.text).toContain("service.install_failed");
  });

  it("returns the page text for failure diagnostics", async () => {
    render(AppShell, { session: new DesktopSession() });
    await settle();
    expect(page<string>("text")).toContain("New task");
  });
});

describe("the journey's page script in the Settings window", () => {
  const refused = (command: string) => ({ ...offline(), message: `not answered in this test: ${command}` });

  function settingsIpc(update: unknown) {
    mockIPC((command) => {
      switch (command) {
        case "watch_settings_navigation":
          return { generation: "1", target: { pane: "general", section: null, plane_id: null } };
        case "watch_local_service":
          return serviceView();
        case "watch_app_update":
          // `null`: the update state can't be read.
          if (update === null) throw { ...offline(), category: "internal", code: "update.state_unavailable" };
          return update;
        case "load_desktop_preferences":
          return { reopenLastTask: true, checkForUpdates: true };
        case "remember_settings_pane":
          return null;
        default:
          throw refused(command);
      }
    });
  }

  it("finds Safety and system and reads the Jet service and App updates blocks", async () => {
    settingsIpc({ revision: 0, currentVersion: "0.2.0", state: { kind: "disabled", reason: "development_build" } });
    render(SettingsApp);
    await settle();
    let view = page<SettingsView>("settings");
    expect(view.panes.map((pane) => pane.name)).toContain("Safety and system");
    expect(view.panes.every((pane) => !pane.disabled)).toBe(true);
    expect(view.pane).toBe("General");
    expect(view.versions).toBeNull();

    const safety = page<HTMLElement | null>("button", {
      scope: { role: "navigation", name: "Settings" },
      names: ["Safety and system"],
    });
    expect(safety?.tagName).toBe("BUTTON");
    await fireEvent.click(safety!);
    await settle();

    view = page<SettingsView>("settings");
    expect(view.pane).toBe("Safety and system");
    expect(view.panes.find((pane) => pane.current)?.name).toBe("Safety and system");
    const service = view.versions!.service!;
    expect(service.facts["Managed by"]).toMatch(/^Managed by this app/);
    expect(service.facts.Status).toBe("Running");
    expect(service.facts.Version).toBe("0.2.0");
    expect(service.facts["Included with this app"]).toBe("0.2.0");
    expect(view.versions!.updates).toBe("This is a development build, so it doesn't update itself.");
    expect(view.versions!.updateControls).toEqual([]);
    expect(page<boolean>("reveal", "Jet service on this computer")).toBe(true);
    expect(page<boolean>("reveal", "No such heading")).toBe(false);
  });

  it("reads an enabled App updates status", async () => {
    settingsIpc({ revision: 0, currentVersion: "0.2.0", state: { kind: "idle", upToDate: true } });
    render(SettingsApp);
    await settle();
    await fireEvent.click(
      page<HTMLElement>("button", { scope: { role: "navigation", name: "Settings" }, names: ["Safety and system"] }),
    );
    await settle();
    const versions = page<SettingsView>("settings").versions!;
    expect(versions.updates).toBe("Jet 0.2.0 is up to date.");
    expect(versions.updateControls).toEqual(["Check for updates", "Check for updates automatically"]);
    expect(updaterOn(versions)).toBe(true);
  });

  it("reads a signed build that can't update itself as off", async () => {
    settingsIpc({ revision: 0, currentVersion: "0.2.0", state: { kind: "disabled", reason: "unsupported_install" } });
    render(SettingsApp);
    await settle();
    await fireEvent.click(
      page<HTMLElement>("button", { scope: { role: "navigation", name: "Settings" }, names: ["Safety and system"] }),
    );
    await settle();
    const versions = page<SettingsView>("settings").versions!;
    expect(versions.updates).toMatch(/^This copy of Jet can't update itself\./);
    expect(versions.updateControls).toEqual([]);
    expect(updaterOn(versions)).toBe(false);
  });

  it("reads a failed update-state read as off", async () => {
    settingsIpc(null);
    render(SettingsApp);
    await settle();
    await fireEvent.click(
      page<HTMLElement>("button", { scope: { role: "navigation", name: "Settings" }, names: ["Safety and system"] }),
    );
    await settle();
    const versions = page<SettingsView>("settings").versions!;
    expect(versions.updates).toMatch(/^Jet couldn't read its update state\./);
    expect(versions.updateControls).toEqual(["Try again"]);
    expect(updaterOn(versions)).toBe(false);
  });
});
