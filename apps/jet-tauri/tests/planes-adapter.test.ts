import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import {
  authorizeApprovalRetry,
  closePlaneFeed,
  interruptTurn,
  loadConversation,
  loadConversations,
  loadRunSupervision,
  loadWorkPanel,
  openPlaneFeed,
  openWorkspaceTerminal,
  searchConversations,
  startRun,
  stopRun,
  submitTurn,
  withdrawTurn,
} from "../src/lib/jet/bridge";
import { loadDeliveries, prepareDelivery, prepareDeliveryAcknowledgement } from "../src/lib/jet/delivery";
import { publicError } from "../src/lib/jet/errors";
import { listPlanes, loadPlaneDetail, planeLabel, type PlanesSnapshot } from "../src/lib/jet/planes";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function recordIpc(): Array<{ command: string; args: Record<string, unknown> }> {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC((command, args) => {
    const { onUpdate: _channel, ...plain } = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: plain });
    return command === "open_plane_feed" ? { state: "online", feedId: "f" } : null;
  });
  return calls;
}

describe("Plane adapter", () => {
  it("sends the exact Plane registry commands", async () => {
    const calls = recordIpc();
    await listPlanes();
    await loadPlaneDetail(REMOTE);
    await closePlaneFeed("feed-1");
    expect(calls).toEqual([
      { command: "list_planes", args: {} },
      { command: "load_plane_detail", args: { planeId: REMOTE } },
      { command: "close_plane_feed", args: { feedId: "feed-1" } },
    ]);
  });

  it("opens a feed for this computer by default and for an explicit Plane on request", async () => {
    const calls = recordIpc();
    await openPlaneFeed(() => undefined, "12");
    await openPlaneFeed(() => undefined, null, REMOTE, true);
    expect(calls).toEqual([
      { command: "open_plane_feed", args: { planeId: null, after: "12", reset: false } },
      { command: "open_plane_feed", args: { planeId: REMOTE, after: null, reset: true } },
    ]);
  });

  it("defaults every scoped wrapper to a null planeId", async () => {
    const calls = recordIpc();
    await loadConversations();
    await searchConversations("fix");
    await loadConversation("c");
    await startRun("c", "codex", "go");
    await submitTurn("c", "more");
    await loadRunSupervision("c", null);
    await withdrawTurn("c", "t");
    await interruptTurn("r");
    await stopRun("r");
    await authorizeApprovalRetry("r", "v");
    await loadWorkPanel("c", "r", { kind: "current" });
    await openWorkspaceTerminal("c");
    await loadDeliveries("c");
    await prepareDelivery("c", { kind: "push", remote: "origin" });
    await prepareDeliveryAcknowledgement("c", "d");
    expect(calls.map(({ command }) => command)).toEqual([
      "load_conversations",
      "search_conversations",
      "load_conversation",
      "start_run",
      "submit_turn",
      "load_run_supervision",
      "withdraw_turn",
      "interrupt_turn",
      "stop_run",
      "authorize_approval_retry",
      "load_work_panel",
      "open_workspace_terminal",
      "load_deliveries",
      "prepare_delivery",
      "prepare_delivery_acknowledgement",
    ]);
    for (const call of calls) expect(call.args.planeId, call.command).toBeNull();
    expect(calls[0].args).toEqual({ planeId: null, nextPage: null });
  });

  it("forwards an explicit planeId on every scoped wrapper", async () => {
    const calls = recordIpc();
    await loadConversations("page-2", REMOTE);
    await searchConversations("fix", REMOTE);
    await loadConversation("c", REMOTE);
    await startRun("c", "codex", "go", REMOTE);
    await submitTurn("c", "more", REMOTE);
    await loadRunSupervision("c", "r", REMOTE);
    await withdrawTurn("c", "t", REMOTE);
    await interruptTurn("r", REMOTE);
    await stopRun("r", REMOTE);
    await authorizeApprovalRetry("r", "v", REMOTE);
    await loadWorkPanel("c", "r", { kind: "turn", turn: 2 }, REMOTE);
    await openWorkspaceTerminal("c", 30, 100, REMOTE);
    await loadDeliveries("c", REMOTE);
    await prepareDelivery("c", { kind: "branch", name: "b" }, REMOTE);
    await prepareDeliveryAcknowledgement("c", "d", REMOTE);
    for (const call of calls) expect(call.args.planeId, call.command).toBe(REMOTE);
    expect(calls[0].args).toEqual({ planeId: REMOTE, nextPage: "page-2" });
    expect(calls[3].args).toEqual({ conversationId: "c", craft: "codex", prompt: "go", planeId: REMOTE });
    expect(calls[10].args).toEqual({
      conversationId: "c",
      runId: "r",
      scopeKind: "turn",
      turn: 2,
      fromTurn: null,
      toTurn: null,
      planeId: REMOTE,
    });
    expect(calls[11].args).toEqual({ conversationId: "c", rows: 30, columns: 100, planeId: REMOTE });
  });

  it("parses protocolLimit and planeId and rejects anything else", () => {
    const base = { category: "incompatible", code: "protocol.feature_unavailable", message: "m", retryable: false };
    expect(
      publicError({ ...base, protocolLimit: { requiredMinor: 12, negotiatedMinor: 9 }, planeId: REMOTE }),
    ).toMatchObject({ protocolLimit: { requiredMinor: 12, negotiatedMinor: 9 }, planeId: REMOTE });
    expect(publicError({ ...base, planeId: "local" }).planeId).toBe("local");
    for (const protocolLimit of [
      { requiredMinor: "12", negotiatedMinor: 9 },
      { requiredMinor: 12.5, negotiatedMinor: 9 },
      { requiredMinor: Number.POSITIVE_INFINITY, negotiatedMinor: 9 },
      { requiredMinor: 12 },
      "12/9",
    ]) {
      expect(publicError({ ...base, protocolLimit }).protocolLimit).toBeNull();
    }
    for (const planeId of ["LOCAL", "/run/jetd.sock", "user@host", REMOTE.toUpperCase(), 7, ""]) {
      expect(publicError({ ...base, planeId }).planeId).toBeNull();
    }
    expect(publicError(base)).toMatchObject({ protocolLimit: null, planeId: null });
  });

  it("labels Planes from the native snapshot only", () => {
    const snapshot = {
      planes: [{ planeId: "local", label: "This computer" }, { planeId: REMOTE, label: "build-box" }],
    } as unknown as PlanesSnapshot;
    expect(planeLabel(snapshot, REMOTE)).toBe("build-box");
    expect(planeLabel(null, "local")).toBe("This computer");
  });
});
