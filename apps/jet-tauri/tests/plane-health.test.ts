import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { ConnectionSnapshot, PlaneUpdate, PublicError } from "../src/lib/jet/bridge";
import { planeConditionFor } from "../src/lib/jet/errors";
import type { PlaneHealthSummary } from "../src/lib/jet/system";
import { DeliverySession } from "../src/lib/features/delivery/session.svelte";
import { PlaneHealth } from "../src/lib/features/system/health.svelte";
import {
  EMPTY_CONDITIONS,
  collectResultText,
  needsRunRecovery,
  noticeFor,
  noticeLink,
  noticeText,
  retentionLine,
} from "../src/lib/features/system/model";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

const HEALTHY: PlaneHealthSummary = { security: "trusted", store: "serving", ledger: "verified" };

function connection(planeId: string, health: PlaneHealthSummary = HEALTHY, daemonStarts = "1"): ConnectionSnapshot {
  return {
    state: "online",
    feedId: `feed-${planeId}`,
    planeId,
    planeIdentity: null,
    health,
    coreVersion: "0.2.0",
    daemonStarts,
    startedAtUnixMs: "1",
    cursor: "40",
  };
}

function refusal(code: string, planeId: string | null = null): PublicError {
  return {
    category: "unavailable",
    code,
    message: "The Plane refused the request.",
    retryable: true,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId,
  };
}

/** A session whose IPC answers the feed and Plane refreshes the updates cause. */
function session(extra: (command: string, args: Record<string, unknown>) => unknown = () => null) {
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC((command, args) => {
    if (command === "list_planes") throw refusal("transport.offline");
    return extra(command, (args ?? {}) as Record<string, unknown>);
  });
  return new DesktopSession();
}

describe("planeConditionFor", () => {
  it("maps exactly the four Plane-wide refusal codes", () => {
    expect(planeConditionFor({ code: "storage.disk_pressure" })).toBe("disk_pressure");
    expect(planeConditionFor({ code: "recovery.read_only" })).toBe("read_only");
    expect(planeConditionFor({ code: "security.audit_degraded" })).toBe("security_degraded");
    expect(planeConditionFor({ code: "recovery.deletion_ledger_corrupt" })).toBe("ledger_corrupt");
    expect(planeConditionFor({ code: "transport.offline" })).toBeNull();
    expect(planeConditionFor({ code: "recovery.not_read_only" })).toBeNull();
    expect(planeConditionFor({ code: "toString" })).toBeNull();
  });
});

describe("Plane-health model", () => {
  it("shows one notice, most severe first", () => {
    const all = {
      summary: { security: "degraded", store: "read_only", ledger: "corrupt" } as const,
      observed: [],
      diskPressureAt: 5,
      dismissed: false,
    };
    expect(noticeFor(all)?.kind).toBe("read_only");
    expect(noticeFor({ ...all, summary: { ...all.summary, store: "serving" } })?.kind).toBe("ledger_corrupt");
    expect(noticeFor({ ...all, summary: HEALTHY, observed: ["security_degraded"] })?.kind).toBe("security_degraded");
    expect(noticeFor({ ...EMPTY_CONDITIONS, diskPressureAt: 5 })).toEqual({ kind: "disk_pressure", diskPressureAt: 5 });
    expect(noticeFor({ ...EMPTY_CONDITIONS, diskPressureAt: 5, dismissed: true })).toBeNull();
    expect(noticeFor({ ...EMPTY_CONDITIONS, summary: HEALTHY })).toBeNull();
  });

  it("names the Plane and links only to landed Settings sections", () => {
    expect(noticeText({ kind: "read_only", diskPressureAt: null }, "Build box")).toBe(
      "Build box is in read-only recovery. Tasks can't change.",
    );
    expect(noticeText({ kind: "disk_pressure", diskPressureAt: 0 }, "Build box")).toMatch(
      /^Low disk space on Build box stopped new work at .+\.$/,
    );
    expect(noticeLink("disk_pressure", REMOTE)).toEqual({
      label: "Open Storage",
      target: { pane: "safety", section: "storage", plane_id: REMOTE },
    });
    expect(noticeLink("security_degraded", "local")).toEqual({
      label: "Review audit",
      target: { pane: "safety", section: "audit", plane_id: "local" },
    });
    for (const kind of ["read_only", "ledger_corrupt"] as const) {
      expect(noticeLink(kind, "local")).toEqual({
        label: "Open Recovery",
        target: { pane: "safety", section: "recovery", plane_id: "local" },
      });
    }
  });

  it("words the collect result and the retention policy", () => {
    expect(collectResultText(0)).toBe("Nothing to remove right now.");
    expect(collectResultText(1)).toBe("Removed 1 file.");
    expect(collectResultText(12)).toBe("Removed 12 files.");
    expect(retentionLine("forget_after_final_run")).toBe("Jet forgets this task after its last activity ends.");
    expect(retentionLine("retain")).toBeNull();
    expect(retentionLine(undefined)).toBeNull();
  });
});

describe("lost-run panel", () => {
  it("shows for a lost Run or when supervision needs attention", () => {
    expect(needsRunRecovery("lost", false)).toBe(true);
    expect(needsRunRecovery("active", true)).toBe(true);
    expect(needsRunRecovery("lost", undefined)).toBe(true);
    expect(needsRunRecovery("active", false)).toBe(false);
    expect(needsRunRecovery("completed", null)).toBe(false);
    expect(needsRunRecovery(undefined, undefined)).toBe(false);
  });
});

describe("PlaneHealth", () => {
  it("observes the four codes per Plane and clears disk pressure on the next success there", () => {
    let now = 1_000;
    const health = new PlaneHealth(() => now);
    health.observe(REMOTE, refusal("storage.disk_pressure"));
    health.observe(REMOTE, refusal("transport.offline"));
    expect(health.notice(REMOTE)).toEqual({ kind: "disk_pressure", diskPressureAt: 1_000 });
    expect(health.notice("local")).toBeNull();

    health.succeeded("local");
    expect(health.notice(REMOTE)?.kind).toBe("disk_pressure");
    health.dismiss(REMOTE);
    expect(health.notice(REMOTE)).toBeNull();
    now = 2_000;
    health.observe(REMOTE, refusal("storage.disk_pressure"));
    expect(health.notice(REMOTE)).toEqual({ kind: "disk_pressure", diskPressureAt: 2_000 });
    health.succeeded(REMOTE);
    expect(health.notice(REMOTE)).toBeNull();

    health.observe("local", refusal("security.audit_degraded"));
    health.observe("local", refusal("recovery.read_only"));
    expect(health.notice("local")?.kind).toBe("read_only");
  });

  it("takes summaries only from online snapshots and resets when the daemon restarts", () => {
    const health = new PlaneHealth();
    expect(health.applyConnection("local", connection("local", { ...HEALTHY, store: "read_only" }))).toBe(false);
    expect(health.notice("local")?.kind).toBe("read_only");
    expect(health.applyConnection("local", { ...connection("local", HEALTHY), state: "reconnecting" })).toBe(false);
    expect(health.notice("local")?.kind).toBe("read_only");

    health.observe("local", refusal("storage.disk_pressure"));
    health.observe("local", refusal("recovery.deletion_ledger_corrupt"));
    // Same daemon start: the fresh summary replaces observed conditions only.
    expect(health.applyConnection("local", connection("local", HEALTHY, "1"))).toBe(false);
    expect(health.notice("local")?.kind).toBe("disk_pressure");
    // A new daemon start drops everything known about the Plane.
    expect(health.applyConnection("local", connection("local", HEALTHY, "2"))).toBe(true);
    expect(health.notice("local")).toBeNull();
  });

  it("clears the Security-degraded condition when a new audit epoch begins", () => {
    const health = new PlaneHealth();
    health.applyConnection(REMOTE, connection(REMOTE, { ...HEALTHY, security: "degraded" }));
    health.observe(REMOTE, refusal("security.audit_degraded"));
    expect(health.notice(REMOTE)?.kind).toBe("security_degraded");
    health.clearSecurity(REMOTE);
    expect(health.notice(REMOTE)).toBeNull();
  });

  it("frees disposable space on the named Plane and ignores a result after the Plane dropped", async () => {
    const calls: Array<[string, unknown]> = [];
    let resolve: (value: { removed: number }) => void = () => {};
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command, args) => {
      calls.push([command, args]);
      return new Promise((done) => (resolve = done));
    });
    const health = new PlaneHealth();
    const first = health.collect(REMOTE);
    expect(health.collectState(REMOTE)).toEqual({ kind: "collecting" });
    await vi.waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0]).toEqual(["collect_disposable_storage", { planeId: REMOTE }]);
    // Single flight: a second click while collecting sends nothing.
    await health.collect(REMOTE);
    expect(calls).toHaveLength(1);
    resolve({ removed: 3 });
    await first;
    expect(health.collectState(REMOTE)).toEqual({ kind: "done", removed: 3 });

    const late = health.collect(REMOTE);
    await vi.waitFor(() => expect(calls).toHaveLength(2));
    health.offline(REMOTE);
    resolve({ removed: 9 });
    await late;
    expect(health.collectState(REMOTE)).toEqual({ kind: "idle" });
  });

  it("records a collect refusal as a Plane condition", async () => {
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC(() => {
      throw refusal("storage.disk_pressure", "local");
    });
    const health = new PlaneHealth(() => 7);
    await health.collect("local");
    expect(health.collectState("local")).toMatchObject({ kind: "failed", error: { code: "storage.disk_pressure" } });
    expect(health.notice("local")).toEqual({ kind: "disk_pressure", diskPressureAt: 7 });
  });
});

describe("DesktopSession Plane health", () => {
  it("applies a summary from connected only; resumed changes nothing", () => {
    const desktop = session();
    const resumed: PlaneUpdate = { type: "resumed", after: "4" };
    desktop.receive("local", resumed);
    expect(desktop.health.conditionsOf("local").summary).toBeNull();
    desktop.receive("local", { type: "connected", connection: connection("local", { ...HEALTHY, security: "degraded" }) });
    expect(desktop.planeHealthNotice?.kind).toBe("security_degraded");
    desktop.receive("local", resumed);
    expect(desktop.planeHealthNotice?.kind).toBe("security_degraded");
    desktop.receive("local", {
      type: "event",
      sequence: "5",
      recorded_at_unix_ms: "1",
      kind: "audit.epoch_begun",
      conversation_id: null,
      run_id: null,
      timeline: [],
    });
    expect(desktop.planeHealthNotice).toBeNull();
  });

  it("a disk-pressure refusal on Plane B shows nothing while a Plane A task is selected", () => {
    const desktop = session();
    desktop.selectedPlaneId = "local";
    desktop.selectedConversationId = "c1";
    desktop.health.observe(REMOTE, refusal("storage.disk_pressure", REMOTE));
    expect(desktop.planeHealthNotice).toBeNull();
    expect(desktop.attentionCount).toBe(0);

    desktop.selectedPlaneId = REMOTE;
    expect(desktop.planeHealthNotice?.kind).toBe("disk_pressure");
    expect(desktop.attentionCount).toBe(1);
  });

  it("Needs attention routes to the notice when a condition exists", () => {
    const desktop = session();
    desktop.select("attention");
    expect(desktop.actionNotice).toBe("No current task needs your attention.");
    expect(desktop.health.focusPending).toBe(false);

    desktop.receive("local", { type: "connected", connection: connection("local", { ...HEALTHY, store: "read_only" }) });
    desktop.select("attention");
    expect(desktop.health.focusPending).toBe(true);
    expect(desktop.actionNotice).toBeNull();
    expect(desktop.attentionCount).toBe(1);
  });

  it("counts activity that needs recovery", () => {
    const desktop = session();
    desktop.supervision = {
      cursor: "1",
      maximumEntries: 128,
      maximumPromptBytes: 65536,
      turns: [],
      execution: {
        cursor: "1",
        run: { id: "r1", conversationId: "c1", revision: "1", lifecycle: "active", title: "Run", createdAtUnixMs: "1", endedAtUnixMs: null },
        activity: null,
        needsAttention: true,
        termination: null,
      },
    };
    expect(desktop.attentionCount).toBe(1);
    desktop.select("attention");
    expect(desktop.actionNotice).toBe("Review the highlighted request and the current Run controls.");
  });

  it("a failed feed marks the Plane offline so a late collect result is ignored", async () => {
    let resolve: (value: { removed: number }) => void = () => {};
    const desktop = session((command) => {
      if (command === "collect_disposable_storage") return new Promise((done) => (resolve = done));
      return null;
    });
    const pending = desktop.health.collect(REMOTE);
    await vi.waitFor(() => expect(desktop.health.collectState(REMOTE).kind).toBe("collecting"));
    desktop.receive(REMOTE, { type: "reconnecting", error: refusal("transport.offline", REMOTE) });
    resolve({ removed: 1 });
    await pending;
    expect(desktop.health.collectState(REMOTE)).toEqual({ kind: "idle" });
  });

  it("delivery outcomes feed the Plane they ran on", async () => {
    const outcomes: Array<[string, string | null]> = [];
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command) => {
      if (command === "prepare_delivery") {
        return { reviewId: "r1", planeId: REMOTE, conversationId: "c1", workingTree: "isolated", operation: { kind: "push", remote: "origin" }, checkpointFiles: null, contentComplete: null };
      }
      if (command === "load_deliveries") return [];
      if (command === "execute_delivery") return { kind: "refused", error: refusal("storage.disk_pressure", REMOTE) };
      return null;
    });
    const delivery = new DeliverySession((planeId, error) => outcomes.push([planeId, error?.code ?? null]));
    delivery.select("c1", REMOTE);
    await delivery.refresh();
    await delivery.prepare({ kind: "push", remote: "origin" });
    await delivery.confirm();
    expect(outcomes).toEqual([[REMOTE, "storage.disk_pressure"]]);
  });
});
