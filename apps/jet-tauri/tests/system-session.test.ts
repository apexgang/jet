import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { withIssues } from "../src/lib/features/settings/model";
import { SettingsSession } from "../src/lib/features/settings/session.svelte";
import { SystemSession } from "../src/lib/features/system/session.svelte";
import type { PublicError } from "../src/lib/jet/bridge";
import { collectDisposableStorage, loadSystemHealth, type SystemHealth } from "../src/lib/jet/system";

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
    recovery: { kind: "serving", snapshotCount: 2, ledger: { kind: "verified", deletions: "0" } },
    security: { kind: "trusted" },
    storage: { disposableMiB: 2048 },
    retention: { graceDays: 30 },
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
});
