import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type { ConnectionSnapshot, PublicError } from "../src/lib/jet/bridge";
import type { LocalServiceView } from "../src/lib/jet/local-service";
import { DEFAULT_PRESENTATION } from "../src/lib/jet/presentation";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import {
  LOCAL_PLANE_CONNECTED,
  SHELL_INTERACTIVE,
  localServiceMark,
  mark,
  markShellInteractive,
} from "../src/lib/features/shell/timing";
import { LocalServiceSession } from "../src/lib/features/system/local-service.svelte";
import { serviceView } from "./support/service";

/** The release journey reads these names (tests/e2e/journey.ts). */
const marks = (name: string) => performance.getEntriesByName(name, "mark").length;

const CONNECTION = {
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

const OFFLINE: PublicError = {
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

beforeEach(() => performance.clearMarks());
afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function settle() {
  for (let round = 0; round < 60; round += 1) await Promise.resolve();
}

describe("release journey timing marks", () => {
  it("marks shell-interactive once, in the frame after mount", () => {
    const frames: Array<() => void> = [];
    markShellInteractive((callback) => frames.push(callback));
    expect(marks(SHELL_INTERACTIVE)).toBe(0);
    frames.shift()!();
    expect(marks(SHELL_INTERACTIVE)).toBe(1);

    // A second mount in the same page never marks again.
    markShellInteractive((callback) => frames.push(callback));
    expect(frames).toHaveLength(0);
    expect(marks(SHELL_INTERACTIVE)).toBe(1);
  });

  it("marks at once where there are no frames", () => {
    vi.stubGlobal("requestAnimationFrame", undefined);
    markShellInteractive();
    expect(marks(SHELL_INTERACTIVE)).toBe(1);
  });

  it("never lets a failing Performance API reach the shell", () => {
    vi.spyOn(performance, "mark").mockImplementation(() => {
      throw new Error("no timeline");
    });
    expect(() => mark("anything")).not.toThrow();
    vi.spyOn(performance, "getEntriesByName").mockImplementation(() => {
      throw new Error("no timeline");
    });
    expect(() => markShellInteractive((callback) => callback())).not.toThrow();
  });

  it("marks each local-service phase a window sees, once per change", async () => {
    let channel: Channel<LocalServiceView> | null = null;
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command, args) => {
      if (command === "watch_local_service") {
        channel = (args as { onChange: Channel<LocalServiceView> }).onChange;
        return serviceView({ phase: "checking" });
      }
      return null;
    });
    const service = new LocalServiceSession();
    await service.start();
    expect(localServiceMark("installing")).toBe("local-service-installing");
    expect(marks("local-service-checking")).toBe(1);
    channel!.onmessage(serviceView({ phase: "installing" }));
    channel!.onmessage(serviceView({ phase: "installing", currentVersion: "0.2.0" }));
    channel!.onmessage(serviceView({ phase: "running", lastAction: "installed" }));
    expect(marks("local-service-installing")).toBe(1);
    expect(marks("local-service-running")).toBe(1);
  });

  it("marks every time this computer's feed comes online, and never for a remote Plane", async () => {
    let feed: Channel<unknown> | null = null;
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command, args) => {
      const record = (args ?? {}) as Record<string, unknown>;
      switch (command) {
        case "open_plane_feed":
          feed = record.onUpdate as Channel<unknown>;
          return CONNECTION;
        case "watch_local_service":
          return serviceView();
        case "load_desktop_preferences":
          return { reopenLastTask: false, checkForUpdates: true };
        case "load_shell_presentation":
          return { presentation: DEFAULT_PRESENTATION, issue: null };
        case "list_planes":
          return {
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
          };
        case "load_conversations":
          return { planeId: "local", cursor: "40", conversations: [], nextPage: null };
        default:
          return null;
      }
    });
    const session = new DesktopSession();
    session.connect();
    await settle();
    // The feed opened online.
    expect(marks(LOCAL_PLANE_CONNECTED)).toBe(1);

    // The page of events that follows says resumed; it marks nothing more.
    feed!.onmessage({ type: "resumed", after: "40" });
    await settle();
    expect(marks(LOCAL_PLANE_CONNECTED)).toBe(1);

    // jetd restarted: the open feed dials again and says resumed, never
    // connected (`client.rs` `stream_updates`).
    feed!.onmessage({ type: "reconnecting", error: OFFLINE });
    feed!.onmessage({ type: "reconnecting", error: OFFLINE });
    await settle();
    expect(marks(LOCAL_PLANE_CONNECTED)).toBe(1);
    feed!.onmessage({ type: "resumed", after: "40" });
    await settle();
    expect(marks(LOCAL_PLANE_CONNECTED)).toBe(2);
    expect(session.connectionState).toBe("online");

    // A feed whose first status read failed comes back with connected.
    feed!.onmessage({ type: "reconnecting", error: OFFLINE });
    feed!.onmessage({ type: "connected", connection: CONNECTION });
    feed!.onmessage({ type: "resumed", after: "40" });
    await settle();
    expect(marks(LOCAL_PLANE_CONNECTED)).toBe(3);

    const remote = "0000000a-0000-4000-8000-000000000002";
    session.receive(remote, { type: "reconnecting", error: { ...OFFLINE, planeId: remote } });
    session.receive(remote, { type: "resumed", after: "40" });
    session.receive(remote, { type: "connected", connection: { ...CONNECTION, planeId: remote } });
    expect(marks(LOCAL_PLANE_CONNECTED)).toBe(3);
    session.disconnect();
  });
});
