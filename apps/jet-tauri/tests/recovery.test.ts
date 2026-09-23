import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type { PlaneUpdate } from "../src/lib/jet/bridge";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

type Feed = { planeId: string | null; after: string | null; reset: boolean; channel: Channel<PlaneUpdate> };

function connection(planeId: string, feedId: string) {
  return {
    state: "online",
    feedId,
    planeId,
    planeIdentity: null,
    health: { security: "trusted", store: "serving" },
    coreVersion: "0.2.0",
    daemonStarts: "1",
    startedAtUnixMs: "1",
    cursor: "40",
  };
}

function ipc(extra: (command: string, args: Record<string, unknown>) => unknown = () => {
  throw new Error("unexpected");
}) {
  const feeds: Feed[] = [];
  const calls: string[] = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC((command, args) => {
    const record = (args ?? {}) as Record<string, unknown>;
    calls.push(command);
    if (command === "open_plane_feed") {
      feeds.push({
        planeId: record.planeId as string | null,
        after: record.after as string | null,
        reset: record.reset as boolean,
        channel: record.onUpdate as Channel<PlaneUpdate>,
      });
      return connection((record.planeId as string | null) ?? "local", `feed-${feeds.length}`);
    }
    if (command === "close_plane_feed") return null;
    return extra(command, record);
  });
  return { feeds, calls };
}

function event(conversationId: string, sequence: string): PlaneUpdate {
  return {
    type: "event",
    sequence,
    recorded_at_unix_ms: "1",
    kind: "turn.input",
    conversation_id: conversationId,
    run_id: null,
    timeline: [{ kind: "user", text: `on ${sequence}`, itemId: `turn-${sequence}`, approval: null }],
  };
}

describe("Plane-bound recovery", () => {
  it("resume_events restarts only the feed of the Plane whose command failed", async () => {
    const { feeds } = ipc();
    const session = new DesktopSession();
    await session.applyWorkRecovery({ type: "resume_events", after: "41" }, REMOTE);
    expect(feeds.map(({ planeId, after, reset }) => ({ planeId, after, reset }))).toEqual([
      { planeId: REMOTE, after: "41", reset: false },
    ]);
    // This computer's connection summary is untouched by another Plane's feed.
    expect(session.connectionState).toBe("connecting");

    await session.applyWorkRecovery({ type: "resume_events", after: "7" });
    expect(feeds[1]).toMatchObject({ planeId: "local", after: "7" });
    expect(session.connectionState).toBe("online");
  });

  it("refresh_conversation for a Plane that is not selected does nothing", async () => {
    const loaded: unknown[] = [];
    const { calls } = ipc((command, args) => {
      if (command === "load_conversation") {
        loaded.push(args);
        return {
          conversation: { planeId: "local", id: "c1", revision: "3", title: "Task", createdAtUnixMs: "1", projectId: null },
          cursor: "40",
          workspaceId: null,
          workspaceRoot: null,
          runs: [],
        };
      }
      if (command === "load_run_supervision") {
        return { cursor: "40", maximumEntries: 128, maximumPromptBytes: 65536, turns: [], execution: null };
      }
      throw new Error(`Unexpected ${command}`);
    });
    const session = new DesktopSession();
    session.selectedConversationId = "c1";
    session.selectedPlaneId = "local";

    await session.applyWorkRecovery({ type: "refresh_conversation" }, REMOTE);
    await session.applyWorkRecovery({ type: "refresh_run" }, REMOTE);
    expect(calls).toEqual([]);

    await session.applyWorkRecovery({ type: "refresh_conversation" }, "local");
    expect(loaded).toEqual([{ conversationId: "c1", planeId: "local" }]);
  });

  it("takes timeline events only from the selected Conversation's Plane", async () => {
    vi.useFakeTimers();
    const { feeds } = ipc();
    const session = new DesktopSession();
    session.selectedConversationId = "c1";
    session.selectedPlaneId = "local";
    await session.applyWorkRecovery({ type: "resume_events", after: "1" }, "local");
    await session.applyWorkRecovery({ type: "resume_events", after: "1" }, REMOTE);
    const [local, remote] = feeds.map((feed) => feed.channel.onmessage);

    // The same Conversation UUID on another Plane is a different task.
    remote(event("c1", "5"));
    expect(session.timeline).toEqual([]);
    remote({ type: "failed", error: { category: "offline", code: "transport.offline", message: "m", retryable: false, recoveryActions: [], restart: null, revisionConflict: null, protocolLimit: null, planeId: REMOTE } });
    expect(session.connectionState).toBe("online");

    local(event("c1", "6"));
    expect(session.timeline.map((entry) => entry.text)).toEqual(["on 6"]);
  });

  it("ignores a replaced feed of the same Plane and closes every feed on disconnect", async () => {
    vi.useFakeTimers();
    const { feeds, calls } = ipc();
    const session = new DesktopSession();
    session.selectedConversationId = "c1";
    await session.applyWorkRecovery({ type: "resume_events", after: "1" }, "local");
    await session.applyWorkRecovery({ type: "resume_events", after: "2" }, "local");
    feeds[0].channel.onmessage(event("c1", "3"));
    expect(session.timeline).toEqual([]);
    feeds[1].channel.onmessage(event("c1", "4"));
    expect(session.timeline).toHaveLength(1);

    session.disconnect();
    await Promise.resolve();
    expect(calls.filter((command) => command === "close_plane_feed")).toHaveLength(1);
    feeds[1].channel.onmessage(event("c1", "5"));
    expect(session.timeline).toHaveLength(1);
  });
});
