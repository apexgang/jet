import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { withIssues } from "../src/lib/features/settings/model";
import { SettingsSession } from "../src/lib/features/settings/session.svelte";
import {
  bytesText,
  isStaleRecoveryError,
  purgeBlock,
  purgeReviewText,
  recoveryHeadline,
  recoveryRefusalText,
  restoreBlock,
  unconfirmedText,
} from "../src/lib/features/system/model";
import { SystemSession } from "../src/lib/features/system/session.svelte";
import type { PublicError } from "../src/lib/jet/bridge";
import {
  collectDisposableStorage,
  executeRecoveryAction,
  loadSystemHealth,
  prepareRecoveryAction,
  type RecoveryReview,
  type RecoveryView,
  type SystemHealth,
} from "../src/lib/jet/system";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function failure(code: string, category = "invalid_input", retryable = false, planeId: string | null = "local"): PublicError {
  return {
    category,
    code,
    message: "Message",
    retryable,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId,
  };
}

function health(planeId: string, daemonStarts = "3", issues: SystemHealth["issues"] = []): SystemHealth {
  return {
    planeId,
    planeLabel: planeId === "local" ? "This computer" : "Build box",
    service: { coreVersion: "1.43.0", daemonStarts, startedAtUnixMs: "1700000000000" },
    app: { version: "0.1.0", supportedProtocol: "1.43" },
    protocol: { exact: null, atLeast: 38, atMost: null },
    platform: "linux · x86_64",
    tools: [],
    crafts: [],
    credentialStore: { state: "available", kind: "secret_service" },
    degraded: [],
    recovery: { kind: "serving", snapshotCount: 2, snapshots: [], ledger: { kind: "verified", deletions: "0" } },
    security: { kind: "trusted" },
    storage: { disposableMiB: 2048 },
    retention: { graceDays: 30 },
    pendingEpoch: false,
    issues,
  };
}

async function settle(): Promise<void> {
  for (let turn = 0; turn < 8; turn++) await new Promise((resolve) => setTimeout(resolve, 0));
}

type Call = { command: string; args: Record<string, unknown> };
type Answer = (args: Record<string, unknown>) => unknown;

/** A fake shell: each command answers from `answers`, or waits until released. */
class Fake {
  calls: Call[] = [];
  answers = new Map<string, Answer>();
  held = new Map<string, Array<() => void>>();
  hold = new Set<string>();

  install(): void {
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      this.calls.push({ command, args });
      const answer = () => {
        const handler = this.answers.get(command);
        if (!handler) throw failure("client.state_unavailable", "internal");
        return handler(args);
      };
      if (!this.hold.has(command)) return answer();
      return new Promise((resolve, reject) => {
        const queue = this.held.get(command) ?? [];
        queue.push(() => {
          try {
            resolve(answer());
          } catch (error) {
            reject(error);
          }
        });
        this.held.set(command, queue);
      });
    });
  }

  release(command: string): void {
    this.held.get(command)?.shift()?.();
  }

  count(command: string): number {
    return this.calls.filter((call) => call.command === command).length;
  }
}

describe("system adapter", () => {
  it("sends the exact command names and arguments", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_system_health", (args) => health(args.planeId as string));
    fake.answers.set("collect_disposable_storage", () => ({ removed: 2 }));
    await loadSystemHealth(REMOTE);
    await loadSystemHealth("local", true);
    await collectDisposableStorage(REMOTE);
    expect(fake.calls).toEqual([
      { command: "load_system_health", args: { planeId: REMOTE, fresh: false } },
      { command: "load_system_health", args: { planeId: "local", fresh: true } },
      { command: "collect_disposable_storage", args: { planeId: REMOTE } },
    ]);
  });
});

describe("system session health", () => {
  it("loads once per Plane and shows partial failures in their own section", async () => {
    const fake = new Fake();
    fake.install();
    const issues: SystemHealth["issues"] = [
      { section: "capabilities", error: failure("capability.observation_failed", "unavailable", true) },
      { section: "retention", error: failure("unauthorized", "unauthorized") },
    ];
    fake.answers.set("load_system_health", (args) => ({ ...health(args.planeId as string), issues }));
    const system = new SystemSession();
    system.select("local");
    await system.reloadIfLoaded();
    expect(fake.count("load_system_health")).toBe(0);

    await system.ensureLoaded();
    await system.ensureLoaded();
    expect(fake.count("load_system_health")).toBe(1);
    expect(system.health.kind).toBe("ready");
    const versions = withIssues(system.health, ["capabilities"]);
    const storage = withIssues(system.health, ["storage"]);
    const diagnostics = withIssues(system.health, ["retention"]);
    expect(versions.kind === "ready" && versions.issues.map((issue) => issue.error.code)).toEqual(["capability.observation_failed"]);
    expect(storage.kind === "ready" && storage.issues).toEqual([]);
    expect(diagnostics.kind === "ready" && diagnostics.issues.map((issue) => issue.error.code)).toEqual(["unauthorized"]);
    // The failed sections' codes feed the diagnostic summary.
    expect(system.recentCodes).toEqual(["capability.observation_failed", "unauthorized"]);

    // Check again asks for a fresh capability observation.
    await system.load(true);
    expect(fake.calls.at(-1)).toEqual({ command: "load_system_health", args: { planeId: "local", fresh: true } });
  });

  it("classifies a fatal status failure and keeps the last health while offline", async () => {
    const fake = new Fake();
    fake.install();
    let answer: Answer = (args) => health(args.planeId as string);
    fake.answers.set("load_system_health", (args) => answer(args));
    const system = new SystemSession();
    system.select(REMOTE);
    await system.ensureLoaded();

    answer = () => {
      throw failure("transport.offline", "offline", true, REMOTE);
    };
    await system.load();
    expect(system.health.kind).toBe("offline");
    expect(system.health.kind === "offline" && system.health.last?.planeLabel).toBe("Build box");

    answer = () => {
      throw failure("unauthorized", "unauthorized", false, REMOTE);
    };
    await system.load();
    expect(system.health.kind).toBe("denied");

    answer = () => {
      throw { ...failure("protocol.feature_unavailable", "unavailable", false, REMOTE), protocolLimit: { requiredMinor: 43, negotiatedMinor: 30 } };
    };
    await system.load();
    expect(system.health.kind).toBe("unsupported");
    expect(system.recentCodes).toEqual(["transport.offline", "unauthorized", "protocol.feature_unavailable"]);
  });

  it("drops a health read that finishes after the Plane changed", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_system_health", (args) => health(args.planeId as string));
    fake.hold.add("load_system_health");
    const system = new SystemSession();
    system.select("local");
    const first = system.ensureLoaded();
    await settle();
    system.select(REMOTE);
    fake.release("load_system_health");
    await first;
    expect(system.planeId).toBe(REMOTE);
    expect(system.health).toEqual({ kind: "loading", last: null });

    fake.hold.clear();
    await system.ensureLoaded();
    expect(system.health.kind === "ready" && system.health.data.planeId).toBe(REMOTE);
  });

  it("a new Jet service start drops what the old one reported", async () => {
    const fake = new Fake();
    fake.install();
    let starts = "3";
    fake.answers.set("load_system_health", (args) => health(args.planeId as string, starts));
    fake.answers.set("collect_disposable_storage", () => ({ removed: 4 }));
    const system = new SystemSession();
    system.select("local");
    await system.ensureLoaded();

    fake.hold.add("collect_disposable_storage");
    const collecting = system.freeSpace();
    await settle();
    expect(system.collect.kind).toBe("collecting");

    starts = "4";
    await system.load();
    expect(system.health.kind === "ready" && system.health.data.service.daemonStarts).toBe("4");
    expect(system.collect.kind).toBe("idle");
    // The pass the old service answered is not shown against the new one.
    fake.release("collect_disposable_storage");
    await collecting;
    expect(system.collect.kind).toBe("idle");
  });

  it("reloads for disclosed settings and Plane events only when loaded", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_system_health", (args) => health(args.planeId as string));
    const system = new SystemSession();
    system.select("local");
    await system.settingChanged("storage.disposable_mib");
    expect(fake.count("load_system_health")).toBe(0);

    await system.ensureLoaded();
    await system.settingChanged("energy.concurrency");
    expect(fake.count("load_system_health")).toBe(1);
    await system.settingChanged("storage.disposable_mib");
    await system.settingChanged("retention.trash_grace_days");
    expect(fake.count("load_system_health")).toBe(3);

    system.markStale();
    expect(system.health.kind === "ready" && system.health.freshness).toBe("stale");
  });
});

describe("free disposable space", () => {
  it("is single-flight and reports its result", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("collect_disposable_storage", () => ({ removed: 3 }));
    fake.hold.add("collect_disposable_storage");
    const system = new SystemSession();
    system.select(REMOTE);
    const first = system.freeSpace();
    const second = system.freeSpace();
    await settle();
    expect(fake.count("collect_disposable_storage")).toBe(1);
    fake.release("collect_disposable_storage");
    await Promise.all([first, second]);
    expect(system.collect).toEqual({ kind: "done", removed: 3 });
  });

  it("shows the Plane's busy refusal and records low-disk refusals with their time", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("collect_disposable_storage", () => {
      throw failure("storage.collect_busy", "conflict", false, REMOTE);
    });
    const system = new SystemSession(() => 1_700_000_000_000);
    system.select(REMOTE);
    await system.freeSpace();
    expect(system.collect.kind === "failed" && system.collect.error.code).toBe("storage.collect_busy");

    system.observe(failure("storage.disk_pressure", "unavailable", true, "local"));
    expect(system.diskPressureAt).toBeNull();
    system.observe(failure("storage.disk_pressure", "unavailable", true, REMOTE));
    expect(system.diskPressureAt).toBe(1_700_000_000_000);
    expect(system.recentCodes).toEqual(["storage.collect_busy", "storage.disk_pressure"]);

    system.select("local");
    expect(system.diskPressureAt).toBeNull();
    expect(system.collect.kind).toBe("idle");
  });
});

describe("Settings session wiring", () => {
  it("forwards Plane events and watcher loss to the health section", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_system_health", (args) => health(args.planeId as string));
    const session = new SettingsSession();
    session.system.select("local");
    await session.system.ensureLoaded();
    expect(fake.count("load_system_health")).toBe(1);

    await session.receive({
      type: "change",
      sequence: "20",
      kind: "audit.epoch_begun",
      settingKey: null,
      settingScope: null,
      projectId: null,
    });
    await settle();
    expect(fake.count("load_system_health")).toBe(2);

    await session.receive({
      type: "change",
      sequence: "21",
      kind: "setting.changed",
      settingKey: "storage.disposable_mib",
      settingScope: "plane",
      projectId: null,
    });
    await settle();
    expect(fake.count("load_system_health")).toBe(3);

    await session.receive({ type: "reconnecting", error: failure("transport.offline", "offline", true) });
    expect(session.system.health.kind === "ready" && session.system.health.freshness).toBe("stale");
    // A failed settings read is recorded for the diagnostic summary.
    await settle();
    expect(session.system.recentCodes).toContain("client.state_unavailable");
    session.dispose();
  });

  it("shows a restarted Jet service in Versions once the watcher resumes", async () => {
    const fake = new Fake();
    fake.install();
    let service = { coreVersion: "0.2.0", daemonStarts: "3" };
    fake.answers.set("load_system_health", (args) => {
      const read = health(args.planeId as string, service.daemonStarts);
      return { ...read, service: { ...read.service, coreVersion: service.coreVersion } };
    });
    const session = new SettingsSession();
    const restarted = vi.spyOn(session.autodelete, "planeRestarted");
    session.system.select("local");
    await session.system.ensureLoaded();

    // A core activation drains jetd: this window's watcher drops, then its
    // redial resumes against the new start (`settings.rs` keeps the redial's
    // status read native; the resumed reloads every section).
    await session.receive({ type: "reconnecting", error: failure("transport.offline", "offline", true) });
    service = { coreVersion: "0.3.0", daemonStarts: "4" };
    await session.receive({ type: "resumed", after: "10" });
    await settle();
    const shown = session.system.health.kind === "ready" ? session.system.health : null;
    expect(shown?.data.service).toMatchObject({ coreVersion: "0.3.0", daemonStarts: "4" });
    expect(shown?.freshness).toBe("live");
    expect(restarted).toHaveBeenCalledTimes(1);
    session.dispose();
  });
});

const TOKEN = "5a1f0000-0000-4000-8000-000000000001";
const REVIEW = "5a1f0000-0000-4000-8000-0000000000aa";

function readOnly(): RecoveryView {
  return {
    kind: "read_only",
    reason: "integrity_check_failed",
    snapshotCount: 1,
    snapshots: [{ snapshotId: TOKEN, takenAtUnixMs: "1700000000000", reason: "daily", bytes: "4096" }],
    ledger: { kind: "verified", deletions: "2" },
  };
}

function restoreReview(reviewId = REVIEW): RecoveryReview {
  return {
    kind: "restore_snapshot",
    reviewId,
    planeLabel: "Build box",
    takenAtUnixMs: "1700000000000",
    reason: "daily",
    bytes: "4096",
  };
}

describe("recovery adapter", () => {
  it("names snapshots only by token and sends only the review ID", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("prepare_recovery_action", () => restoreReview());
    fake.answers.set("execute_recovery_action", () => ({ kind: "purged", removedCount: 0 }));
    await prepareRecoveryAction(REMOTE, { kind: "restore_snapshot", snapshot_id: TOKEN });
    await prepareRecoveryAction("local", { kind: "purge_snapshots" });
    await executeRecoveryAction(REMOTE, REVIEW);
    expect(fake.calls).toEqual([
      {
        command: "prepare_recovery_action",
        args: { planeId: REMOTE, action: { kind: "restore_snapshot", snapshot_id: TOKEN } },
      },
      { command: "prepare_recovery_action", args: { planeId: "local", action: { kind: "purge_snapshots" } } },
      { command: "execute_recovery_action", args: { planeId: REMOTE, reviewId: REVIEW } },
    ]);
  });
});

describe("system session recovery", () => {
  function readOnlyHealth(starts = "3"): SystemHealth {
    return { ...health(REMOTE, starts), recovery: readOnly(), security: { kind: "absent" } };
  }

  it("reviews, restores, then drops everything read before and reloads", async () => {
    const fake = new Fake();
    fake.install();
    let current = readOnlyHealth("3");
    fake.answers.set("load_system_health", () => current);
    fake.answers.set("prepare_recovery_action", () => restoreReview());
    fake.answers.set("execute_recovery_action", () => {
      current = health(REMOTE, "4");
      return { kind: "restored", takenAtUnixMs: "1700000000000", reason: "daily", replacedName: "plane.sqlite3.damaged-1" };
    });
    let restarts = 0;
    let restored = 0;
    const system = new SystemSession(
      Date.now,
      () => restarts++,
      () => restored++,
    );
    system.select(REMOTE);
    await system.ensureLoaded();

    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    expect(system.recovery).toEqual({ kind: "review", review: restoreReview() });
    await system.confirmRecovery();

    expect(system.recovery.kind === "done" && system.recovery.outcome.kind).toBe("restored");
    // One restart for the restore itself; the reload's new start count is not a second one.
    expect(restarts).toBe(1);
    expect(restored).toBe(1);
    expect(fake.count("load_system_health")).toBe(2);
    expect(system.health.kind === "ready" && system.health.data.recovery.kind).toBe("serving");
    // A second confirm has nothing to send.
    await system.confirmRecovery();
    expect(fake.count("execute_recovery_action")).toBe(1);
  });

  it("an unconfirmed restore re-reads the Plane and never resends", async () => {
    const fake = new Fake();
    fake.install();
    let current = readOnlyHealth("3");
    fake.answers.set("load_system_health", () => current);
    fake.answers.set("prepare_recovery_action", () => restoreReview());
    fake.answers.set("execute_recovery_action", () => ({
      kind: "unconfirmed",
      error: failure("transport.offline", "offline", true, REMOTE),
    }));
    let restored = 0;
    const system = new SystemSession(Date.now, () => undefined, () => restored++);
    system.select(REMOTE);
    await system.ensureLoaded();

    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    await system.confirmRecovery();
    expect(system.recovery).toMatchObject({ kind: "unconfirmed", check: "read_only" });
    expect(fake.count("load_system_health")).toBe(2);
    expect(fake.count("execute_recovery_action")).toBe(1);
    expect(system.recentCodes).toContain("transport.offline");
    expect(restored).toBe(0);

    // Retrying means a new review, prepared from a fresh read.
    system.closeRecovery();
    fake.answers.set("prepare_recovery_action", () => restoreReview("5a1f0000-0000-4000-8000-0000000000bb"));
    current = health(REMOTE, "4");
    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    fake.answers.set("execute_recovery_action", () => {
      throw failure("client.state_unavailable", "internal", true, REMOTE);
    });
    await system.confirmRecovery();
    // A thrown error is unconfirmed too; this read shows the store serving.
    expect(system.recovery).toMatchObject({ kind: "unconfirmed", check: "serving" });
    expect(restored).toBe(1);
    const sent = fake.calls.filter((call) => call.command === "execute_recovery_action").map((call) => call.args.reviewId);
    expect(sent).toEqual([REVIEW, "5a1f0000-0000-4000-8000-0000000000bb"]);
  });

  it("a local precondition that no longer holds is stale with Reload; a Plane refusal is refused", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_system_health", () => readOnlyHealth());
    fake.answers.set("prepare_recovery_action", () => {
      throw failure("recovery.snapshot_gone", "conflict", false, REMOTE);
    });
    const system = new SystemSession();
    system.select(REMOTE);
    await system.ensureLoaded();

    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    expect(system.recovery.kind === "stale" && system.recovery.error.code).toBe("recovery.snapshot_gone");
    await system.reloadRecovery();
    expect(system.recovery.kind).toBe("closed");
    expect(fake.count("load_system_health")).toBe(2);

    fake.answers.set("prepare_recovery_action", () => restoreReview());
    fake.answers.set("execute_recovery_action", () => ({
      kind: "refused",
      error: failure("recovery.restore_failed", "unavailable", false, REMOTE),
    }));
    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    await system.confirmRecovery();
    expect(system.recovery.kind === "refused" && system.recovery.error.code).toBe("recovery.restore_failed");
    expect(fake.count("load_system_health")).toBe(3);

    fake.answers.set("execute_recovery_action", () => ({
      kind: "refused",
      error: failure("client.review_used", "invalid_input", false, REMOTE),
    }));
    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    await system.confirmRecovery();
    expect(system.recovery.kind).toBe("stale");
  });

  it("drops a review that finishes after the Plane changed or the dialog closed", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_system_health", (args) => health(args.planeId as string));
    fake.answers.set("prepare_recovery_action", () => restoreReview());
    fake.hold.add("prepare_recovery_action");
    const system = new SystemSession();
    system.select(REMOTE);

    const late = system.prepareRecovery({ kind: "purge_snapshots" });
    await settle();
    expect(system.recovery).toEqual({ kind: "preparing", action: "purge_snapshots", snapshotId: null });
    // A second click while preparing sends nothing.
    await system.prepareRecovery({ kind: "purge_snapshots" });
    expect(fake.count("prepare_recovery_action")).toBe(1);
    system.closeRecovery();
    fake.release("prepare_recovery_action");
    await late;
    expect(system.recovery.kind).toBe("closed");

    const other = system.prepareRecovery({ kind: "purge_snapshots" });
    await settle();
    system.closeRecovery();
    // A restore names the one snapshot row being checked.
    const restore = system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    await settle();
    expect(system.recovery).toEqual({ kind: "preparing", action: "restore_snapshot", snapshotId: TOKEN });
    system.select("local");
    fake.release("prepare_recovery_action");
    fake.release("prepare_recovery_action");
    await other;
    await restore;
    expect(system.recovery.kind).toBe("closed");
  });

  it("a sending review cannot be closed and a restart only replaces an open review", async () => {
    const fake = new Fake();
    fake.install();
    let starts = "3";
    fake.answers.set("load_system_health", () => ({ ...health(REMOTE, starts), recovery: readOnly() }));
    fake.answers.set("prepare_recovery_action", () => restoreReview());
    fake.answers.set("execute_recovery_action", () => ({ kind: "purged", removedCount: 2 }));
    const system = new SystemSession();
    system.select(REMOTE);
    await system.ensureLoaded();

    await system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: TOKEN });
    starts = "4";
    await system.load();
    expect(system.recovery.kind).toBe("restarted");
    await system.confirmRecovery();
    expect(fake.count("execute_recovery_action")).toBe(0);

    system.closeRecovery();
    await system.prepareRecovery({ kind: "purge_snapshots" });
    fake.hold.add("execute_recovery_action");
    const sending = system.confirmRecovery();
    await settle();
    system.closeRecovery();
    expect(system.recovery.kind).toBe("sending");
    fake.release("execute_recovery_action");
    await sending;
    expect(system.recovery).toMatchObject({ kind: "done", outcome: { kind: "purged", removedCount: 2 } });
  });
});

describe("recovery copy", () => {
  it("states the store, the snapshots and why an action is unavailable", () => {
    const serving: RecoveryView = {
      kind: "serving",
      snapshotCount: 1,
      snapshots: [{ snapshotId: TOKEN, takenAtUnixMs: "1700000000000", reason: "daily", bytes: "4096" }],
      ledger: { kind: "verified", deletions: "2" },
    };
    expect(recoveryHeadline(serving, "Build box")).toBe(
      "Your Jet data on Build box is healthy. Jet keeps 1 verified recovery snapshot.",
    );
    expect(recoveryHeadline(readOnly(), "Build box")).toBe(
      "Jet found a problem with its data on Build box (the data check failed). You can read tasks, but nothing can change until you restore a snapshot.",
    );
    expect(recoveryHeadline({ kind: "unsupported" }, "Build box")).toMatch(/doesn't report recovery snapshots/);

    expect(restoreBlock(readOnly())).toBeNull();
    expect(restoreBlock({ ...readOnly(), ledger: { kind: "corrupt" } } as RecoveryView)).toMatch(/won't restore a snapshot/);
    expect(restoreBlock(serving)).toBeNull();

    expect(purgeBlock(serving, { kind: "trusted" }, "Build box")).toBeNull();
    expect(purgeBlock(serving, { kind: "absent" }, "Build box")).toMatch(/security audit/);
    expect(purgeBlock({ ...serving, ledger: { kind: "unsupported" } }, { kind: "trusted" }, "Build box")).toMatch(
      /Update Jet on that Plane/,
    );
    expect(purgeBlock(readOnly(), { kind: "trusted" }, "Build box")).toBeNull();
  });

  it("words sizes, the purge consequence and the unconfirmed checks", () => {
    expect(bytesText("1")).toBe("1 byte");
    expect(bytesText("512")).toBe("512 bytes");
    expect(bytesText("4096")).toBe("4.1 KB");
    expect(bytesText("405504000")).toBe("406 MB");
    expect(bytesText("-1")).toBe("Size unknown");
    expect(
      purgeReviewText({
        kind: "purge_snapshots",
        reviewId: REVIEW,
        planeLabel: "Build box",
        snapshotCount: 3,
        totalBytes: "3000000",
        deletionsRecorded: "2",
        includesRollback: true,
      }),
    ).toBe(
      "Remove snapshots that may still contain deleted tasks? Jet takes a new snapshot now and then removes older ones, possibly all 3 (3 MB), including rollback copies for the previous version. Removed snapshots can't be recovered.",
    );
    expect(unconfirmedText(restoreReview(), "serving")).toBe("The store is serving again. The restore likely completed.");
    expect(unconfirmedText(restoreReview(), "read_only")).toBe("The Plane is still read-only. Review a snapshot again to retry.");
  });

  it("treats local preconditions as stale and explains refusals without native text", () => {
    for (const code of ["recovery.snapshot_gone", "recovery.not_read_only_local", "recovery.purge_unavailable", "client.review_used"]) {
      expect(isStaleRecoveryError({ code }), code).toBe(true);
    }
    expect(isStaleRecoveryError({ code: "recovery.restore_failed" })).toBe(false);
    expect(recoveryRefusalText({ code: "recovery.not_read_only", message: "m" }, "Build box")).toBe(
      "Build box isn't in read-only recovery now. Reload to see its current state.",
    );
    expect(recoveryRefusalText({ code: "unknown.code", message: "Fixed shell message." }, "Build box")).toBe(
      "Fixed shell message.",
    );
  });
});
