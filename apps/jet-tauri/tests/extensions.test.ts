import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import {
  changeStateText,
  entryActionLabel,
  isTerminalChange,
  offeredActions,
  type ExtensionEntry,
} from "../src/lib/features/settings/extensions-model";
import { ExtensionsSession } from "../src/lib/features/settings/extensions-session.svelte";
import { publicError } from "../src/lib/jet/errors";
import {
  inspectExtension,
  loadExtensionCatalog,
  loadExtensionChange,
  prepareExtensionChange,
  type ExtensionCatalogView,
  type ExtensionChangeState,
  type ExtensionInspection,
} from "../src/lib/jet/extensions";
import type { SettingsReceipt, SettingsReview } from "../src/lib/jet/settings";

const REMOTE = "0000000a-0000-4000-8000-000000000002";
const TOKEN = "00000000-0000-4000-8000-00000000e001";
const INSPECTION = "00000000-0000-4000-8000-00000000e002";
const CHANGE = "00000000-0000-4000-8000-00000000c001";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function ipc(handler: Parameters<typeof mockIPC>[0]) {
  vi.stubGlobal("window", {});
  mockIPC(handler);
}

function failure(code: string, category = "invalid_input", retryable = false) {
  return { category, code, message: "Message", retryable, recoveryActions: [], restart: null, revisionConflict: null, protocolLimit: null, planeId: null };
}

async function settle(): Promise<void> {
  for (let turn = 0; turn < 12; turn++) await Promise.resolve();
}

function catalog(overrides: Partial<ExtensionCatalogView> = {}): ExtensionCatalogView {
  return {
    craftId: "codex",
    harness: "Codex",
    standalone: [{ entryToken: TOKEN, id: "skill:a", enabled: true }],
    plugins: [{ entryToken: "00000000-0000-4000-8000-00000000e003", id: "linear@curated", installed: true }],
    changes: [],
    truncated: false,
    issues: [],
    ...overrides,
  };
}

function inspection(overrides: Partial<ExtensionInspection> = {}): ExtensionInspection {
  return {
    inspectionId: INSPECTION,
    extensionId: "skill:a",
    publisher: "user-selected native source",
    version: null,
    source: "/home/u/.agents/skills/a",
    files: [{ path: "SKILL.md", sha256: "a".repeat(64) }],
    fileCount: 1,
    disabled: false,
    supportedActions: null,
    reviewable: true,
    ...overrides,
  };
}

function review(reviewId = "r-1"): SettingsReview {
  return {
    reviewId,
    subject: {
      kind: "change_extension",
      preview: {
        craftId: "codex",
        harness: "Codex",
        extensionId: "skill:a",
        action: "remove",
        publisher: null,
        version: null,
        source: null,
        files: [{ path: "SKILL.md", sha256: "a".repeat(64) }],
        fileCount: 1,
      },
    },
    before: null,
    after: null,
  };
}

const STANDALONE: ExtensionEntry = { kind: "standalone", entryToken: TOKEN, id: "skill:a", enabled: true };

describe("extensions adapter", () => {
  it("names entries by token and changes by inspection ID, never by catalog JSON", async () => {
    const calls: Array<[string, unknown]> = [];
    ipc((command, args) => {
      calls.push([command, args]);
      return {};
    });
    await loadExtensionCatalog(REMOTE, "codex");
    await inspectExtension(REMOTE, TOKEN);
    await prepareExtensionChange(REMOTE, INSPECTION, "disable");
    await loadExtensionChange(REMOTE, CHANGE);
    expect(calls).toEqual([
      ["load_extension_catalog", { planeId: REMOTE, craftId: "codex" }],
      ["inspect_extension", { planeId: REMOTE, entryToken: TOKEN }],
      ["prepare_extension_change", { planeId: REMOTE, inspectionId: INSPECTION, action: "disable" }],
      ["load_extension_change", { planeId: REMOTE, changeId: CHANGE }],
    ]);
    // Nothing but opaque IDs and the action crosses back to the shell.
    const sent = JSON.stringify(calls);
    for (const native of ["extensionId", "extension_id", "catalog\":", "native_metadata", "files", "skill:a"]) {
      expect(sent).not.toContain(native);
    }
  });

  it("keeps a thrown PublicError intact", async () => {
    ipc(() => {
      throw failure("extensions.inspection_expired");
    });
    let error = null;
    try {
      await inspectExtension("local", TOKEN);
    } catch (thrown: unknown) {
      error = publicError(thrown);
    }
    expect(error?.code).toBe("extensions.inspection_expired");
  });
});

describe("offered actions", () => {
  it("offers standalone entries only disable, remove or restoring install", () => {
    expect(offeredActions(STANDALONE)).toEqual(["disable", "remove"]);
    const off: ExtensionEntry = { ...STANDALONE, enabled: false };
    expect(offeredActions(off)).toEqual(["install", "remove"]);
    expect(entryActionLabel(off, "install")).toBe("Turn back on");
    // A standalone inspection's hint never widens what a standalone row offers.
    expect(offeredActions(STANDALONE, { supportedActions: ["install", "update"] })).toEqual(["disable", "remove"]);
  });

  it("offers plugins what the Craft supports, else by installation state", () => {
    const plugin = (installed: boolean | null): ExtensionEntry => ({ kind: "plugin", entryToken: TOKEN, id: "p@m", installed });
    expect(offeredActions(plugin(true))).toEqual(["update", "disable", "remove"]);
    expect(offeredActions(plugin(false))).toEqual(["install"]);
    expect(offeredActions(plugin(null))).toEqual(["install", "update", "disable", "remove"]);
    expect(offeredActions(plugin(false), { supportedActions: ["disable", "remove"] })).toEqual(["disable", "remove"]);
    expect(offeredActions(plugin(true), { supportedActions: [] })).toEqual([]);
    expect(entryActionLabel(plugin(false), "install")).toBe("Install");
  });

  it("names change states and which are final", () => {
    const states: ExtensionChangeState[] = ["staged", "applied", "refused", "outcome_unknown"];
    expect(states.map(isTerminalChange)).toEqual([false, true, true, true]);
    expect(isTerminalChange("checking")).toBe(false);
    expect(changeStateText("staged", "Codex")).toBe("Waiting for running tasks");
    expect(changeStateText("outcome_unknown", "Codex")).toBe(
      "Outcome unknown: check Codex's settings before trying again",
    );
  });
});

/** A fake shell for the Extensions session. */
class Fake {
  calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  catalog = catalog();
  inspection = inspection();
  receipt: () => SettingsReceipt = () => ({ kind: "applied", detail: { kind: "extension_change_queued", changeId: CHANGE } });
  states: ExtensionChangeState[] = ["staged", "staged", "applied"];
  deferred = new Map<string, Array<(value: unknown) => void>>();
  defer = new Set<string>();

  install(): void {
    ipc((command, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      this.calls.push({ command, args });
      const answer = () => this.answer(command);
      if (this.defer.has(command)) {
        return new Promise((resolve) => {
          const queue = this.deferred.get(command) ?? [];
          queue.push(() => resolve(answer()));
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

  private answer(command: string): unknown {
    switch (command) {
      case "load_extension_catalog":
        return this.catalog;
      case "inspect_extension":
        return this.inspection;
      case "prepare_extension_change":
        return review();
      case "apply_settings_change":
        return this.receipt();
      case "load_extension_change": {
        const state = this.states.shift() ?? "applied";
        return { changeId: CHANGE, craftId: "codex", extensionId: "skill:a", action: "remove", state };
      }
      default:
        throw new Error(`unexpected ${command}`);
    }
  }
}

function session(fake: Fake, stale = vi.fn()): ExtensionsSession {
  fake.install();
  const extensions = new ExtensionsSession({ planeStateStale: stale }, { visible: () => true, pollMs: 2_000 });
  extensions.select("local");
  return extensions;
}

describe("extensions session", () => {
  it("inspects by token, reviews by inspection ID and tracks the queued change until it is final", async () => {
    vi.useFakeTimers();
    const fake = new Fake();
    const extensions = session(fake);
    await extensions.ensureLoaded(["codex"]);
    await extensions.inspect("codex", "Codex", STANDALONE);
    expect(extensions.inspection.kind).toBe("ready");
    await extensions.prepare("remove");
    expect(extensions.operation.kind).toBe("confirm");
    await extensions.confirm();
    expect(extensions.operation).toMatchObject({ kind: "queued", changeId: CHANGE });
    expect(extensions.inspection.kind).toBe("none");
    expect(fake.calls.find((call) => call.command === "inspect_extension")?.args).toEqual({ planeId: "local", entryToken: TOKEN });
    expect(fake.calls.find((call) => call.command === "prepare_extension_change")?.args).toEqual({
      planeId: "local",
      inspectionId: INSPECTION,
      action: "remove",
    });
    expect(fake.calls.find((call) => call.command === "apply_settings_change")?.args).toEqual({
      planeId: "local",
      reviewId: "r-1",
    });

    // Polled every two seconds until a final state, then never again.
    await settle();
    expect(extensions.changes[0].state).toBe("staged");
    await vi.advanceTimersByTimeAsync(2_000);
    expect(extensions.changes[0].state).toBe("staged");
    await vi.advanceTimersByTimeAsync(2_000);
    expect(extensions.changes[0].state).toBe("applied");
    const reads = fake.count("load_extension_change");
    expect(reads).toBe(3);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fake.count("load_extension_change")).toBe(reads);
    // The catalog is read again to show the result.
    expect(fake.count("load_extension_catalog")).toBeGreaterThanOrEqual(3);
    extensions.forget(CHANGE);
    expect(extensions.changes).toEqual([]);
  });

  it("stops polling an unknown outcome and never retries the change", async () => {
    vi.useFakeTimers();
    const fake = new Fake();
    fake.states = ["outcome_unknown"];
    const extensions = session(fake);
    fake.catalog = catalog({ changes: [{ changeId: CHANGE, extensionId: "skill:a", action: "remove" }] });
    await extensions.ensureLoaded(["codex"]);
    await settle();
    expect(extensions.changes).toMatchObject([{ changeId: CHANGE, harness: "Codex", state: "outcome_unknown" }]);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fake.count("load_extension_change")).toBe(1);
    expect(fake.count("apply_settings_change")).toBe(0);
  });

  it("waits while the window is hidden and stops when the Plane changes", async () => {
    vi.useFakeTimers();
    const fake = new Fake();
    fake.install();
    let visible = false;
    const extensions = new ExtensionsSession({ planeStateStale: vi.fn() }, { visible: () => visible, pollMs: 2_000 });
    extensions.select("local");
    fake.catalog = catalog({ changes: [{ changeId: CHANGE, extensionId: "skill:a", action: "remove" }] });
    await extensions.ensureLoaded(["codex"]);
    await vi.advanceTimersByTimeAsync(6_000);
    expect(fake.count("load_extension_change")).toBe(0);
    visible = true;
    await vi.advanceTimersByTimeAsync(2_000);
    expect(fake.count("load_extension_change")).toBe(1);
    extensions.select(REMOTE);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fake.count("load_extension_change")).toBe(1);
    expect(extensions.changes).toEqual([]);
  });

  it("drops a catalog or inspection that completes after a Plane switch", async () => {
    const fake = new Fake();
    fake.defer.add("load_extension_catalog");
    fake.defer.add("inspect_extension");
    const extensions = session(fake);
    const loading = extensions.loadCatalog("codex");
    const inspecting = extensions.inspect("codex", "Codex", STANDALONE);
    extensions.select(REMOTE);
    fake.release("load_extension_catalog");
    fake.release("inspect_extension");
    await Promise.all([loading, inspecting]);
    expect(extensions.catalogs).toEqual({});
    expect(extensions.inspection).toEqual({ kind: "none" });
  });

  it("keeps an uncertain change and retries it with the same review ID", async () => {
    const fake = new Fake();
    let fail = true;
    fake.receipt = () => {
      if (fail) throw failure("transport.offline", "offline", true);
      return { kind: "applied", detail: { kind: "extension_change_queued", changeId: CHANGE } };
    };
    const extensions = session(fake);
    await extensions.inspect("codex", "Codex", STANDALONE);
    await extensions.prepare("remove");
    await extensions.confirm();
    expect(extensions.operation).toMatchObject({ kind: "uncertain", reviewId: "r-1" });
    // Closing Details keeps the uncertain change; nothing new may start.
    extensions.closeInspection();
    expect(extensions.operation.kind).toBe("uncertain");
    expect(extensions.idle).toBe(false);
    // It survives a Plane switch and back.
    extensions.select(REMOTE);
    expect(extensions.operation.kind).toBe("idle");
    extensions.select("local");
    expect(extensions.operation.kind).toBe("uncertain");
    fail = false;
    await extensions.retry();
    const applies = fake.calls.filter((call) => call.command === "apply_settings_change").map((call) => call.args.reviewId);
    expect(applies).toEqual(["r-1", "r-1"]);
    expect(extensions.operation).toMatchObject({ kind: "queued", changeId: CHANGE });
    extensions.dispose();
  });

  it("does not review an entry whose files can't be shown", async () => {
    const fake = new Fake();
    fake.inspection = inspection({ reviewable: false, files: [], fileCount: 0 });
    const extensions = session(fake);
    await extensions.inspect("codex", "Codex", STANDALONE);
    await extensions.prepare("remove");
    expect(fake.count("prepare_extension_change")).toBe(0);
    expect(extensions.operation.kind).toBe("idle");
  });

  it("reports a refusal and refreshes Plane state for a degraded audit", async () => {
    const fake = new Fake();
    fake.receipt = () => ({ kind: "refused", error: publicError(failure("security.audit_degraded", "conflict")) });
    const stale = vi.fn();
    const extensions = session(fake, stale);
    await extensions.inspect("codex", "Codex", STANDALONE);
    await extensions.prepare("remove");
    await extensions.confirm();
    expect(extensions.operation).toMatchObject({ kind: "refused", error: { code: "security.audit_degraded" } });
    expect(stale).toHaveBeenCalledOnce();
    expect(extensions.changes).toEqual([]);
  });

  it("reloads the catalog when an entry token expired", async () => {
    const fake = new Fake();
    const extensions = session(fake);
    fake.install();
    ipc((command) => {
      fake.calls.push({ command, args: {} });
      if (command === "inspect_extension") throw failure("extensions.inspection_expired");
      return catalog();
    });
    await extensions.inspect("codex", "Codex", STANDALONE);
    await settle();
    expect(extensions.inspection).toMatchObject({ kind: "failed", error: { code: "extensions.inspection_expired" } });
    expect(fake.count("load_extension_catalog")).toBe(1);
  });
});
