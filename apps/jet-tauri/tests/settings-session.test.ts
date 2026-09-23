import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import { rowKey, SettingsSession } from "../src/lib/features/settings/session.svelte";
import { publicError } from "../src/lib/jet/errors";
import {
  loadSettings,
  prepareSettingChange,
  type PlaneState,
  type ResolvedSetting,
  type SettingChangePreparation,
  type SettingKeyId,
  type SettingsChange,
  type SettingsReceipt,
  type SettingsSnapshot,
  type SettingValue,
} from "../src/lib/jet/settings";

const REMOTE = "0000000a-0000-4000-8000-000000000002";
const PLANE = { type: "plane" } as const;

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function failure(code: string, category = "invalid_input", retryable = false) {
  return { category, code, message: "Message", retryable, recoveryActions: [], restart: null, revisionConflict: null, protocolLimit: null, planeId: null };
}

async function settle(): Promise<void> {
  for (let turn = 0; turn < 12; turn++) await new Promise((resolve) => setTimeout(resolve, 0));
}

type Call = { command: string; args: Record<string, unknown> };

/**
 * A fake shell for one or two Planes. Each Plane keeps values and a cursor;
 * tests override prepare/apply or defer any command.
 */
class Fake {
  calls: Call[] = [];
  channels: Array<{ planeId: string; after: string; channel: Channel<SettingsChange> }> = [];
  cursor = 10;
  /** The Agents view's cursor; `null` until a test sets it. */
  agentsCursor: number | null = null;
  planeState: PlaneState = { security: "trusted", recovery: "serving" };
  values = new Map<SettingKeyId, ResolvedSetting>([
    ["energy.constrained", { key: "energy.constrained", value: { type: "flag", value: false }, source: { source: "built_in" } }],
    ["energy.concurrency", { key: "energy.concurrency", value: { type: "count", value: 8 }, source: { source: "built_in" } }],
    ["retention.trash_grace_days", { key: "retention.trash_grace_days", value: { type: "count", value: 30 }, source: { source: "built_in" } }],
  ]);
  private snapshots = 0;
  private reviews = 0;
  /** Commands whose next call waits until the test resolves it. */
  deferred = new Map<string, Array<(value: unknown) => void>>();
  defer = new Set<string>();
  prepare: (args: Record<string, unknown>) => SettingChangePreparation = (args) => this.review(args);
  apply: (args: Record<string, unknown>) => SettingsReceipt = (args) => this.applied(args);
  private pendingChanges = new Map<string, { key: SettingKeyId; change: { kind: string; value?: SettingValue } }>();

  install(): void {
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      this.calls.push({ command, args });
      const answer = () => this.answer(command, args);
      if (this.defer.has(command)) {
        this.defer.delete(command);
        return new Promise((resolve, reject) => {
          const queue = this.deferred.get(command) ?? [];
          queue.push(() => {
            try {
              resolve(answer());
            } catch (error) {
              reject(error);
            }
          });
          this.deferred.set(command, queue);
        });
      }
      return answer();
    });
  }

  release(command: string): void {
    this.deferred.get(command)?.shift()?.(undefined);
  }

  count(command: string): number {
    return this.calls.filter((call) => call.command === command).length;
  }

  last(command: string): Record<string, unknown> {
    const calls = this.calls.filter((call) => call.command === command);
    return calls[calls.length - 1].args;
  }

  snapshot(planeId: string): SettingsSnapshot {
    return {
      snapshotId: `${planeId}-snapshot-${++this.snapshots}`,
      cursor: String(this.cursor),
      scope: { type: "plane" },
      planeState: this.planeState,
      settings: [...this.values.values()],
    };
  }

  review(args: Record<string, unknown>): SettingChangePreparation {
    const key = args.key as SettingKeyId;
    const change = args.change as { kind: string; value?: SettingValue };
    const reviewId = `review-${++this.reviews}`;
    this.pendingChanges.set(reviewId, { key, change });
    return {
      kind: "review",
      review: {
        reviewId,
        subject: { kind: "setting", key, scope: { type: "plane" } },
        before: this.values.get(key) ?? null,
        after: change.kind === "set" ? (change.value ?? null) : null,
      },
    };
  }

  applied(args: Record<string, unknown>): SettingsReceipt {
    const pending = this.pendingChanges.get(args.reviewId as string);
    if (!pending) throw new Error("unknown review");
    this.cursor += 1;
    if (pending.change.kind === "set" && pending.change.value) {
      this.values.set(pending.key, { key: pending.key, value: pending.change.value, source: { source: "plane" } });
    }
    return { kind: "applied", detail: { kind: "setting", key: pending.key, value: pending.change.value ?? null } };
  }

  send(change: SettingsChange): void {
    this.channels[this.channels.length - 1].channel.onmessage(change);
  }

  settingChange(sequence: number, key: SettingKeyId, scope: "plane" | "project" = "plane"): SettingsChange {
    return { type: "change", sequence: String(sequence), kind: "setting.changed", settingKey: key, settingScope: scope, projectId: null };
  }

  private answer(command: string, args: Record<string, unknown>): unknown {
    switch (command) {
      case "load_settings":
        return this.snapshot(args.planeId as string);
      case "load_work_context":
        return {
          planeState: this.planeState,
          projects: [],
          projectsCursor: String(this.cursor),
          bindings: [],
          autodelete: { count: 0 },
          issues: [],
        };
      case "prepare_setting_change":
        return this.prepare(args);
      case "apply_settings_change":
        return this.apply(args);
      case "load_agents":
        return {
          planeState: this.planeState,
          crafts: [],
          harnesses: [],
          credentialStore: null,
          degraded: [],
          accounts: [],
          bindOptions: [],
          usage: null,
          cursor: String(this.agentsCursor ?? this.cursor),
          issues: [],
        };
      case "watch_settings_changes":
        this.channels.push({ planeId: args.planeId as string, after: args.after as string, channel: args.onChange as Channel<SettingsChange> });
        return null;
      default:
        throw new Error(`Unexpected ${command}`);
    }
  }
}

async function started(fake: Fake, planeId = "local"): Promise<SettingsSession> {
  fake.install();
  const session = new SettingsSession();
  session.select(planeId, planeId === "local" ? "This computer" : "Build box");
  await settle();
  return session;
}

const flag = (value: boolean) => ({ kind: "set" as const, value: { type: "flag" as const, value } });

describe("settings adapter", () => {
  it("sends typed arguments and keeps thrown errors public", async () => {
    const fake = new Fake();
    fake.install();
    await loadSettings("local", { type: "project", project_id: "p1" });
    expect(fake.last("load_settings")).toEqual({ planeId: "local", scope: { type: "project", project_id: "p1" } });
    fake.prepare = () => {
      throw failure("settings.scope_invalid");
    };
    const refused = await prepareSettingChange("local", "s1", "git.auto_push", flag(true)).then(
      () => {
        throw new Error("expected a refusal");
      },
      (error: unknown) => publicError(error),
    );
    expect(refused.code).toBe("settings.scope_invalid");
    expect(fake.last("prepare_setting_change")).toEqual({
      planeId: "local",
      snapshotId: "s1",
      key: "git.auto_push",
      change: { kind: "set", value: { type: "flag", value: true } },
    });
  });
});

describe("settings session", () => {
  it("loads the Plane, then watches from the lowest section cursor", async () => {
    const fake = new Fake();
    const session = await started(fake);
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "live" });
    expect(session.planeState).toEqual({ security: "trusted", recovery: "serving" });
    expect(fake.last("load_settings")).toEqual({ planeId: "local", scope: PLANE });
    expect(fake.channels.map(({ planeId, after }) => ({ planeId, after }))).toEqual([{ planeId: "local", after: "10" }]);
    expect(session.watch).toBe("live");
  });

  it("drops a stale loadSettings reply after a Plane switch", async () => {
    const fake = new Fake();
    fake.install();
    const session = new SettingsSession();
    fake.defer.add("load_settings");
    session.select("local");
    await settle();
    session.select(REMOTE, "Build box");
    await settle();
    expect(session.plane.kind === "ready" && session.plane.data.snapshotId).toMatch(new RegExp(`^${REMOTE}`));
    fake.release("load_settings");
    await settle();
    expect(session.plane.kind === "ready" && session.plane.data.snapshotId).toMatch(new RegExp(`^${REMOTE}`));
    expect(session.planeId).toBe(REMOTE);
  });

  it("applies a plain change at once and reloads the section", async () => {
    const fake = new Fake();
    const session = await started(fake);
    await session.change("energy.constrained", PLANE, flag(true));
    await settle();
    expect(fake.last("apply_settings_change")).toEqual({ planeId: "local", reviewId: "review-1" });
    expect(session.row("energy.constrained", PLANE)).toEqual({ kind: "idle" });
    expect(session.setting("energy.constrained", PLANE)?.value).toEqual({ type: "flag", value: true });
  });

  it("stops a sensitive change at confirm until the user confirms", async () => {
    const fake = new Fake();
    const session = await started(fake);
    await session.change("retention.trash_grace_days", PLANE, { kind: "set", value: { type: "count", value: 7 } });
    expect(session.row("retention.trash_grace_days", PLANE).kind).toBe("confirm");
    expect(fake.count("apply_settings_change")).toBe(0);
    session.dismiss("retention.trash_grace_days", PLANE);
    expect(session.row("retention.trash_grace_days", PLANE)).toEqual({ kind: "idle" });
    await session.change("retention.trash_grace_days", PLANE, { kind: "set", value: { type: "count", value: 7 } });
    await session.confirm("retention.trash_grace_days", PLANE);
    expect(fake.count("apply_settings_change")).toBe(1);
    expect(session.row("retention.trash_grace_days", PLANE)).toEqual({ kind: "idle" });
  });

  it("shows a value changed before the prepare and keeps the draft", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const current: ResolvedSetting = { key: "energy.constrained", value: { type: "flag", value: true }, source: { source: "plane" } };
    fake.prepare = () => ({ kind: "changed", current });
    await session.change("energy.constrained", PLANE, flag(false));
    expect(session.row("energy.constrained", PLANE)).toEqual({
      kind: "changed_elsewhere",
      current,
      draft: { type: "flag", value: false },
    });
    expect(fake.count("apply_settings_change")).toBe(0);
  });

  it("shows a value changed between confirm and apply", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const current: ResolvedSetting = { key: "energy.constrained", value: { type: "flag", value: true }, source: { source: "plane" } };
    fake.apply = () => ({ kind: "changed", current });
    await session.change("energy.constrained", PLANE, flag(false));
    expect(session.row("energy.constrained", PLANE)).toEqual({
      kind: "changed_elsewhere",
      current,
      draft: { type: "flag", value: false },
    });
  });

  it("reloads once on an expired snapshot and prepares the same draft again", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const loads = fake.count("load_settings");
    let prepares = 0;
    fake.prepare = (args) => {
      if (++prepares === 1) throw failure("settings.snapshot_expired");
      return fake.review(args);
    };
    await session.change("energy.constrained", PLANE, flag(true));
    await settle();
    const prepared = fake.calls.filter((call) => call.command === "prepare_setting_change").map((call) => call.args);
    expect(prepared).toHaveLength(2);
    expect(prepared[0].snapshotId).not.toBe(prepared[1].snapshotId);
    expect(prepared[1].change).toEqual(prepared[0].change);
    expect(fake.count("load_settings")).toBeGreaterThan(loads);
    expect(fake.count("apply_settings_change")).toBe(1);
  });

  it("retries an uncertain apply with the same review ID", async () => {
    const fake = new Fake();
    const session = await started(fake);
    let applies = 0;
    fake.apply = (args) => {
      if (++applies === 1) throw failure("transport.offline", "offline", true);
      return fake.applied(args);
    };
    await session.change("energy.constrained", PLANE, flag(true));
    expect(session.row("energy.constrained", PLANE)).toMatchObject({ kind: "uncertain", reviewId: "review-1" });
    // No other change may start on that row meanwhile.
    await session.change("energy.constrained", PLANE, flag(false));
    expect(fake.count("prepare_setting_change")).toBe(1);
    await session.retry("energy.constrained", PLANE);
    const applied = fake.calls.filter((call) => call.command === "apply_settings_change").map((call) => call.args);
    expect(applied).toEqual([
      { planeId: "local", reviewId: "review-1" },
      { planeId: "local", reviewId: "review-1" },
    ]);
    expect(session.row("energy.constrained", PLANE)).toEqual({ kind: "idle" });
  });

  it("ends the review on a refusal; a degraded audit also reloads Plane state", async () => {
    const fake = new Fake();
    const session = await started(fake);
    fake.apply = () => ({ kind: "refused", error: failure("setting.value_below_minimum") });
    await session.change("energy.constrained", PLANE, flag(true));
    expect(session.row("energy.constrained", PLANE)).toMatchObject({ kind: "refused", error: { code: "setting.value_below_minimum" } });

    const loads = fake.count("load_settings");
    fake.planeState = { security: "degraded", recovery: "serving" };
    fake.apply = () => ({ kind: "refused", error: failure("security.audit_degraded", "conflict") });
    await session.change("energy.constrained", PLANE, flag(true));
    await settle();
    expect(session.row("energy.constrained", PLANE)).toMatchObject({ kind: "refused", error: { code: "security.audit_degraded" } });
    expect(fake.count("load_settings")).toBe(loads + 1);
    expect(session.planeState?.security).toBe("degraded");
  });

  it("never reports its own apply as a change from elsewhere", async () => {
    const fake = new Fake();
    const session = await started(fake);
    fake.defer.add("apply_settings_change");
    const change = session.change("energy.constrained", PLANE, flag(true));
    await settle();
    expect(session.row("energy.constrained", PLANE).kind).toBe("applying");
    // The event of this very change arrives before the receipt.
    fake.cursor = 11;
    fake.send(fake.settingChange(11, "energy.constrained"));
    await settle();
    expect(session.row("energy.constrained", PLANE).kind).toBe("applying");
    fake.cursor = 10;
    fake.release("apply_settings_change");
    await change;
    await settle();
    expect(session.row("energy.constrained", PLANE)).toEqual({ kind: "idle" });
    // And again after the reload: its sequence is within the new cursor.
    fake.send(fake.settingChange(11, "energy.constrained"));
    await settle();
    expect(session.row("energy.constrained", PLANE)).toEqual({ kind: "idle" });
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "live" });
  });

  it("ignores a change at or below the section cursor", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const loads = fake.count("load_settings");
    fake.send(fake.settingChange(10, "energy.constrained"));
    await settle();
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "live" });
    fake.send(fake.settingChange(11, "energy.constrained"));
    await settle();
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "changed" });
    expect(fake.count("load_settings")).toBe(loads);
  });

  it("marks an edited row changed elsewhere after a reload shows a new value", async () => {
    const fake = new Fake();
    const session = await started(fake);
    session.edit("energy.concurrency", PLANE, { type: "count", value: 4 });
    fake.cursor = 11;
    fake.values.set("energy.concurrency", { key: "energy.concurrency", value: { type: "count", value: 6 }, source: { source: "plane" } });
    fake.send(fake.settingChange(11, "energy.concurrency"));
    await settle();
    expect(session.row("energy.concurrency", PLANE)).toEqual({
      kind: "changed_elsewhere",
      current: { key: "energy.concurrency", value: { type: "count", value: 6 }, source: { source: "plane" } },
      draft: { type: "count", value: 4 },
    });

    // "Apply my value again" is a new prepare, never automatic.
    const prepares = fake.count("prepare_setting_change");
    await session.applyAgain("energy.concurrency", PLANE);
    expect(fake.count("prepare_setting_change")).toBe(prepares + 1);
    expect(fake.last("prepare_setting_change").change).toEqual({ kind: "set", value: { type: "count", value: 4 } });
  });

  it("keeps an edit when another key changed", async () => {
    const fake = new Fake();
    const session = await started(fake);
    session.edit("energy.concurrency", PLANE, { type: "count", value: 4 });
    fake.cursor = 11;
    fake.send(fake.settingChange(11, "energy.constrained"));
    await settle();
    expect(session.row("energy.concurrency", PLANE).kind).toBe("editing");
    expect(session.plane).toMatchObject({ freshness: "changed" });
  });

  it("shows a usage notice without reloading", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const calls = fake.calls.length;
    fake.send({ type: "change", sequence: "11", kind: "usage.recorded", settingKey: null, settingScope: null, projectId: null });
    fake.send({ type: "change", sequence: "12", kind: "schedule.fired", settingKey: null, settingScope: null, projectId: null });
    await settle();
    expect(session.usageNewer).toBe(true);
    expect(session.schedulesChanged).toBe(true);
    expect(fake.calls.length).toBe(calls);
  });

  it("disables every change during read-only recovery", async () => {
    const fake = new Fake();
    fake.planeState = { security: "trusted", recovery: "read_only" };
    const session = await started(fake);
    expect(session.mutationBlock(PLANE)).toBe("read_only");
    await session.change("energy.constrained", PLANE, flag(true));
    expect(fake.count("prepare_setting_change")).toBe(0);
    expect(session.row("energy.constrained", PLANE)).toEqual({ kind: "idle" });
  });

  it("keeps stale values while reconnecting and reloads on resume", async () => {
    const fake = new Fake();
    const session = await started(fake);
    await session.agents.ensureLoaded();
    expect(session.agents.view).toMatchObject({ kind: "ready", freshness: "live" });
    fake.send({ type: "reconnecting", error: failure("transport.offline", "offline", true) });
    await settle();
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "stale" });
    // Every section says why its changes are paused, Agents included.
    expect(session.agents.view).toMatchObject({ kind: "ready", freshness: "stale" });
    expect(session.mutationBlock(PLANE)).toBe("stale");
    expect(session.agentsBlock()).toBe("stale");
    await session.change("energy.constrained", PLANE, flag(true));
    expect(fake.count("prepare_setting_change")).toBe(0);

    const loads = fake.count("load_settings");
    const agentLoads = fake.count("load_agents");
    fake.cursor = 14;
    fake.agentsCursor = 12;
    fake.send({ type: "resumed", after: "10" });
    await settle();
    expect(fake.count("load_settings")).toBe(loads + 1);
    expect(fake.count("load_agents")).toBe(agentLoads + 1);
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "live" });
    expect(session.agents.view).toMatchObject({ kind: "ready", freshness: "live", data: { cursor: "12" } });
    expect(session.mutationBlock(PLANE)).toBeNull();
    expect(session.agentsBlock()).toBeNull();
    // The lowest cursor, the Agents view's included.
    expect(fake.channels[fake.channels.length - 1].after).toBe("12");
  });

  it("sends nothing from an open review once the watcher reconnects", async () => {
    const fake = new Fake();
    const session = await started(fake);
    await session.change("retention.trash_grace_days", PLANE, { kind: "set", value: { type: "count", value: 7 } });
    expect(session.row("retention.trash_grace_days", PLANE).kind).toBe("confirm");
    fake.send({ type: "reconnecting", error: failure("transport.offline", "offline", true) });
    await settle();
    await session.confirm("retention.trash_grace_days", PLANE);
    expect(fake.count("apply_settings_change")).toBe(0);
    expect(session.row("retention.trash_grace_days", PLANE).kind).toBe("confirm");
  });

  it("keeps the draft of an uncertain change, so a changed value re-applies it", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const current: ResolvedSetting = { key: "energy.concurrency", value: { type: "count", value: 6 }, source: { source: "plane" } };
    let applies = 0;
    fake.apply = () => {
      // Nothing was sent the first time; by the retry the value changed.
      if (++applies === 1) throw failure("transport.offline", "offline", true);
      return { kind: "changed", current };
    };
    await session.change("energy.concurrency", PLANE, { kind: "set", value: { type: "count", value: 4 } });
    expect(session.row("energy.concurrency", PLANE)).toMatchObject({
      kind: "uncertain",
      draft: { type: "count", value: 4 },
    });
    await session.retry("energy.concurrency", PLANE);
    expect(session.row("energy.concurrency", PLANE)).toEqual({
      kind: "changed_elsewhere",
      current,
      draft: { type: "count", value: 4 },
    });
    await session.applyAgain("energy.concurrency", PLANE);
    expect(fake.last("prepare_setting_change").change).toEqual({ kind: "set", value: { type: "count", value: 4 } });
  });

  it("shows a late uncertain answer when the user is back on its Plane", async () => {
    const fake = new Fake();
    const session = await started(fake);
    fake.apply = () => {
      throw failure("transport.offline", "offline", true);
    };
    fake.defer.add("apply_settings_change");
    const change = session.change("energy.constrained", PLANE, flag(true));
    await settle();
    expect(session.row("energy.constrained", PLANE).kind).toBe("applying");
    session.select(REMOTE, "Build box");
    await settle();
    expect(session.rows).toEqual({});
    session.select("local", "This computer");
    await settle();
    // Still being sent: no other change may start on the row.
    expect(session.row("energy.constrained", PLANE)).toMatchObject({ kind: "applying", reviewId: "review-1" });
    fake.release("apply_settings_change");
    await change;
    expect(session.row("energy.constrained", PLANE)).toMatchObject({ kind: "uncertain", reviewId: "review-1" });
  });

  it("keeps a late uncertain answer for a Plane not shown", async () => {
    const fake = new Fake();
    const session = await started(fake);
    fake.apply = () => {
      throw failure("transport.offline", "offline", true);
    };
    fake.defer.add("apply_settings_change");
    const change = session.change("energy.constrained", PLANE, flag(true));
    await settle();
    session.select(REMOTE, "Build box");
    await settle();
    fake.release("apply_settings_change");
    await change;
    expect(session.rows).toEqual({});
    session.select("local", "This computer");
    await settle();
    expect(session.row("energy.constrained", PLANE)).toMatchObject({ kind: "uncertain", reviewId: "review-1" });
  });

  it("replays a change event that arrived while its section reloaded", async () => {
    const fake = new Fake();
    const session = await started(fake);
    // The snapshot was read before another device's write at 11.
    fake.defer.add("load_settings");
    const reload = session.loadPlane();
    expect(session.plane.kind).toBe("loading");
    fake.send(fake.settingChange(11, "energy.constrained"));
    await settle();
    fake.release("load_settings");
    await reload;
    await settle();
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "changed" });

    // An event the reloaded snapshot already covers is not reported.
    fake.defer.add("load_settings");
    const again = session.loadPlane();
    fake.send(fake.settingChange(12, "energy.constrained"));
    fake.cursor = 12;
    fake.release("load_settings");
    await again;
    await settle();
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "live" });
  });

  it("ignores messages from a replaced watcher", async () => {
    const fake = new Fake();
    const session = await started(fake);
    const first = fake.channels[0].channel;
    session.select(REMOTE, "Build box");
    await settle();
    first.onmessage(fake.settingChange(50, "energy.constrained"));
    await settle();
    expect(session.plane).toMatchObject({ kind: "ready", freshness: "live" });
    expect(fake.channels[fake.channels.length - 1].planeId).toBe(REMOTE);
  });

  it("keeps an uncertain row across a Plane switch", async () => {
    const fake = new Fake();
    const session = await started(fake);
    fake.apply = () => {
      throw failure("transport.offline", "offline", true);
    };
    await session.change("energy.constrained", PLANE, flag(true));
    expect(session.row("energy.constrained", PLANE).kind).toBe("uncertain");
    session.edit("energy.concurrency", PLANE, { type: "count", value: 3 });
    session.select(REMOTE, "Build box");
    await settle();
    expect(session.rows).toEqual({});
    session.select("local", "This computer");
    await settle();
    expect(session.rows).toEqual({
      [rowKey("energy.constrained", PLANE)]: expect.objectContaining({ kind: "uncertain", reviewId: "review-1" }),
    });
  });
});
