import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type { ConnectionSnapshot, PublicError, SetupSnapshot } from "../src/lib/jet/bridge";
import type { LocalServiceView } from "../src/lib/jet/local-service";
import type { PlanesSnapshot } from "../src/lib/jet/planes";
import { DEFAULT_PRESENTATION } from "../src/lib/jet/presentation";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import { serviceView } from "./support/service";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

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

const CONNECTION: ConnectionSnapshot = {
  state: "online",
  feedId: "feed-1",
  planeId: "local",
  planeIdentity: null,
  health: { security: "trusted", store: "serving" },
  coreVersion: "0.2.0",
  daemonStarts: "1",
  startedAtUnixMs: "1",
  cursor: "40",
} as ConnectionSnapshot;

function setup(projects: SetupSnapshot["projects"]): SetupSnapshot {
  return {
    plane: { coreVersion: "0.2.0", daemonStarts: "1", platform: "linux" },
    capabilities: {
      harnesses: ["codex"],
      crafts: [],
      credentialStore: "available",
      credentialStoreLabel: "Keyring",
      degraded: [],
      authProviders: [],
    },
    projects,
    accounts: [{ id: "a1", label: "Codex", provider: "openai", state: "ready", stateLabel: "Ready" }],
    pairing: { gate: "closed", pairedClients: 0, offerPending: false },
    issues: [],
  };
}

const PLANES: PlanesSnapshot = {
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
  identity: {
    clientId: "00000000-0000-4000-8000-00000000000c",
    key: "unknown",
    fingerprint: null,
    notice: null,
  },
  restoredSelection: null,
  notice: null,
  maximumRemotePlanes: 16,
} as PlanesSnapshot;

type Options = {
  /** `load_setup` answers in order; the last repeats. `offline` rejects. */
  setups: Array<SetupSnapshot | "offline">;
  service: LocalServiceView;
  /** The local feed opens while the service is still down. */
  feedDown?: boolean;
};

function harness(options: Options) {
  const calls: string[] = [];
  const feeds: Array<Channel<unknown>> = [];
  let serviceChannel: Channel<LocalServiceView> | null = null;
  const setups = [...options.setups];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const record = (args ?? {}) as Record<string, unknown>;
    calls.push(command);
    switch (command) {
      case "watch_local_service":
        serviceChannel = record.onChange as Channel<LocalServiceView>;
        return options.service;
      case "repair_local_service":
        return serviceView({ lastAction: "started" });
      case "load_setup": {
        const next = setups.length > 1 ? setups.shift()! : setups[0];
        if (next === "offline") throw offline();
        return next;
      }
      case "open_plane_feed":
        feeds.push(record.onUpdate as Channel<unknown>);
        return options.feedDown ? { ...CONNECTION, state: "reconnecting" } : CONNECTION;
      case "list_planes":
        return PLANES;
      case "load_desktop_preferences":
        return { reopenLastTask: false, checkForUpdates: true };
      case "load_shell_presentation":
        return { presentation: DEFAULT_PRESENTATION, issue: null };
      case "load_conversations":
        return { planeId: "local", cursor: "40", conversations: [], nextPage: null };
      default:
        return null;
    }
  });
  return {
    calls,
    setupReads: () => calls.filter((command) => command === "load_setup").length,
    pushService: (view: LocalServiceView) => serviceChannel?.onmessage(view),
    pushFeed: (update: unknown) => feeds[0]?.onmessage(update),
  };
}

async function settle() {
  for (let tick = 0; tick < 60; tick += 1) await Promise.resolve();
}

describe("Setup and the local Jet service (wave 4 §A)", () => {
  it("re-reads Setup when the local Plane reconnects", async () => {
    const ipc = harness({ setups: [setup([{ id: "p1", name: "Jet", root: "/w" }])], service: serviceView() });
    const session = new DesktopSession();
    session.connect();
    await settle();
    expect(ipc.setupReads()).toBe(1);

    ipc.pushFeed({ type: "reconnecting", error: offline() });
    await settle();
    expect(session.connectionState).toBe("reconnecting");
    ipc.pushFeed({ type: "connected", connection: CONNECTION });
    await settle();
    expect(ipc.setupReads()).toBe(2);
    session.disconnect();
  });

  it("re-reads a failed Setup when the local feed opens online", async () => {
    const ipc = harness({
      setups: ["offline", setup([{ id: "p1", name: "Jet", root: "/w" }])],
      service: serviceView(),
    });
    const session = new DesktopSession();
    session.connect();
    await settle();
    // The feed opened online after startup's failed read.
    expect(session.setup.kind).toBe("ready");
    expect(ipc.setupReads()).toBe(2);
    session.disconnect();
  });

  it("shows provisioning, then reads Setup and opens it when this computer is new", async () => {
    const ipc = harness({
      setups: ["offline", setup([])],
      service: serviceView({ phase: "installing", currentVersion: null, runningVersion: null }),
      feedDown: true,
    });
    const session = new DesktopSession();
    session.connect();
    await settle();
    expect(session.setup.kind).toBe("failed");
    expect(session.service.provisioning).toBe(true);
    // Still provisioning: the startup destination is left alone.
    expect(session.sidebarSelection).not.toBe("project");

    ipc.pushService(serviceView({ lastAction: "installed" }));
    await settle();
    expect(session.setup.kind).toBe("ready");
    expect(session.sidebarSelection).toBe("project");
    session.disconnect();
  });

  it("opens Setup, where Repair is, when the service settles without running", async () => {
    const ipc = harness({
      setups: ["offline"],
      service: serviceView({ phase: "checking" }),
      feedDown: true,
    });
    const session = new DesktopSession();
    session.connect();
    await settle();
    ipc.pushService(serviceView({ phase: "failed", canRepair: true, error: offline() }));
    await settle();
    expect(session.sidebarSelection).toBe("project");

    await session.service.repair();
    await settle();
    expect(ipc.calls).toContain("repair_local_service");
    // Running again: Setup is read once more.
    expect(session.service.view?.phase).toBe("running");
    session.disconnect();
  });

  it("leaves the user's own navigation alone", async () => {
    const ipc = harness({ setups: ["offline"], service: serviceView({ phase: "checking" }), feedDown: true });
    const session = new DesktopSession();
    session.connect();
    await settle();
    session.select("planes");
    ipc.pushService(serviceView({ phase: "failed", canRepair: true }));
    await settle();
    expect(session.sidebarSelection).toBe("planes");
    session.disconnect();
  });
});
