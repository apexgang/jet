import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { AgentsSession } from "../src/lib/features/settings/agents-session.svelte";
import {
  craftTitle,
  historyRows,
  policyFromDraft,
  quotaText,
  receiptText,
  releaseProblem,
  retryDraft,
} from "../src/lib/features/settings/agents-model";
import {
  AUTO_CONTINUE_LIMITS,
  discoverCraft,
  loadAgents,
  loadUsageHistory,
  pickLocalCraftSource,
  prepareAccountBind,
  prepareAccountUnbind,
  prepareAutoContinue,
  prepareCraftDisable,
  type AgentsView,
  type QuotaWindowView,
} from "../src/lib/jet/agents";
import { publicError } from "../src/lib/jet/errors";
import type { SettingsReceipt, SettingsReview } from "../src/lib/jet/settings";

const BINDING = "00000000-0000-4000-8000-0000000000b1";
const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function ipc(handler: Parameters<typeof mockIPC>[0]) {
  vi.stubGlobal("window", {});
  mockIPC(handler);
}

function failure(code: string, category = "invalid_input", retryable = false) {
  return { category, code, message: "Message", retryable, recoveryActions: [], restart: null, revisionConflict: null, protocolLimit: null, planeId: null };
}

async function settle(): Promise<void> {
  for (let turn = 0; turn < 12; turn++) await new Promise((resolve) => setTimeout(resolve, 0));
}

function view(cursor = "10", accounts: AgentsView["accounts"] = []): AgentsView {
  return {
    planeState: { security: "trusted", recovery: "serving" },
    crafts: [{ craftId: "codex", version: "1.4.2", harnesses: [{ id: "codex", name: "Codex" }] }],
    harnesses: [{ id: "codex", name: "Codex" }],
    credentialStore: { state: "available", label: "Secure storage ready" },
    degraded: [],
    accounts,
    bindOptions: [{ provider: "openai", harness: "Codex" }],
    usage: null,
    cursor,
    issues: [],
  };
}

function bindReview(reviewId = "r-1"): SettingsReview {
  return { reviewId, subject: { kind: "bind", provider: "openai", harness: "Codex" }, before: null, after: null };
}

describe("agents adapter", () => {
  it("sends typed arguments with snake_case only inside inputs", async () => {
    const calls: Array<[string, unknown]> = [];
    ipc((command, args) => {
      calls.push([command, args]);
      if (command === "pick_local_craft_source") return null;
      return {};
    });
    await loadAgents("local", true);
    await prepareAccountBind(REMOTE, "openai");
    await loadUsageHistory("local", null, 30);
    await prepareAutoContinue("local", BINDING, {
      mode: "retry",
      delay_ms: 1000,
      max_delay_ms: 60_000,
      max_retries: 3,
      message: "Continue.",
    });
    await prepareAccountUnbind("local", BINDING);
    await prepareCraftDisable("local", "codex", "force");
    expect(await pickLocalCraftSource("local")).toBeNull();
    await discoverCraft("local", { type: "local", source_token: "t-1" });
    await discoverCraft("local", { type: "github_release", repository: "openai/codex", tag: "v1" });
    expect(calls).toEqual([
      ["load_agents", { planeId: "local", freshCredentials: true }],
      ["prepare_account_bind", { planeId: REMOTE, provider: "openai" }],
      ["load_usage_history", { planeId: "local", bindingId: null, days: 30 }],
      [
        "prepare_auto_continue",
        {
          planeId: "local",
          bindingId: BINDING,
          policy: { mode: "retry", delay_ms: 1000, max_delay_ms: 60_000, max_retries: 3, message: "Continue." },
        },
      ],
      ["prepare_account_unbind", { planeId: "local", bindingId: BINDING }],
      ["prepare_craft_disable", { planeId: "local", craftId: "codex", mode: "force" }],
      ["pick_local_craft_source", { planeId: "local" }],
      ["discover_craft", { planeId: "local", source: { type: "local", source_token: "t-1" } }],
      ["discover_craft", { planeId: "local", source: { type: "github_release", repository: "openai/codex", tag: "v1" } }],
    ]);
    // A local source is a native token: no path ever crosses.
    const local = calls.find(([command]) => command === "discover_craft")?.[1] as { source: object };
    expect(Object.keys(local.source).sort()).toEqual(["source_token", "type"]);
  });

  it("keeps thrown errors public", async () => {
    ipc(() => {
      throw failure("agents.local_source_remote");
    });
    const error = await pickLocalCraftSource(REMOTE).catch((thrown: unknown) => publicError(thrown));
    expect(error).toMatchObject({ code: "agents.local_source_remote", category: "invalid_input" });
  });

  it("never imports the dialog plugin in the Settings window", () => {
    const sources = import.meta.glob(
      ["../src/lib/features/settings/*", "../src/lib/jet/agents.ts", "../src/lib/jet/settings.ts"],
      { query: "?raw", import: "default", eager: true },
    ) as Record<string, string>;
    expect(Object.keys(sources).length).toBeGreaterThan(10);
    expect(Object.keys(sources)).toContain("../src/lib/features/settings/CraftInstallDialog.svelte");
    for (const [file, source] of Object.entries(sources)) {
      expect(source, file).not.toContain("@tauri-apps/plugin-dialog");
    }
  });
});

describe("agents model", () => {
  it("mirrors the native Auto-continue bounds", () => {
    expect(AUTO_CONTINUE_LIMITS).toEqual({
      minDelayMs: 1,
      maxDelayMs: 86_400_000,
      minRetries: 1,
      maxRetries: 100,
      maxMessageBytes: 8_192,
    });
    const draft = { mode: "retry" as const, delaySeconds: "300", maxDelaySeconds: "3600", maxRetries: "3", message: "Go on." };
    expect(policyFromDraft(draft)).toEqual({
      policy: { mode: "retry", delay_ms: 300_000, max_delay_ms: 3_600_000, max_retries: 3, message: "Go on." },
    });
    expect(policyFromDraft({ mode: "off" })).toEqual({ policy: { mode: "off" } });
    expect(policyFromDraft(retryDraft())).toHaveProperty("policy");
    for (const invalid of [
      { ...draft, delaySeconds: "0" },
      { ...draft, maxDelaySeconds: "86400.001" },
      { ...draft, delaySeconds: "7200" },
      { ...draft, maxRetries: "0" },
      { ...draft, maxRetries: "101" },
      { ...draft, maxRetries: "2.5" },
      { ...draft, message: "   " },
      { ...draft, message: "é".repeat(4097) },
      { ...draft, message: "bell\u0007" },
    ]) {
      expect(policyFromDraft(invalid), JSON.stringify(invalid)).toHaveProperty("problem");
    }
    expect(policyFromDraft({ ...draft, message: "é".repeat(4096) })).toHaveProperty("policy");
    expect(policyFromDraft({ ...draft, delaySeconds: "0.001", maxDelaySeconds: "86400" })).toHaveProperty("policy");
  });

  it("describes quota windows without the provider's reason", () => {
    const window: QuotaWindowView = {
      provider: "openai",
      window: "5h",
      scope: { kind: "account" },
      unit: "share",
      used: "62",
      limit: "100",
      windowSeconds: "18000",
      resetsAtUnixMs: null,
      estimation: "measured",
      finality: "final",
      observedAtUnixMs: "1",
      freshness: "fresh",
    };
    expect(quotaText(window)).toBe("5h: 62% used");
    expect(quotaText({ ...window, freshness: "stale", estimation: "estimated" })).toBe("5h: 62% used (Stale, estimated)");
    expect(quotaText({ ...window, freshness: "unreachable" })).toBe("5h: Provider didn't answer");
    expect(quotaText({ ...window, unit: "tokens", limit: null, used: "12345", scope: { kind: "model", model: "gpt-5" } })).toBe(
      "5h (gpt-5): 12,345 tokens used",
    );
  });

  it("sums history buckets across models, newest first", () => {
    const point = (start: string, input: string) => ({
      startUnixMs: start,
      tokens: { input, cachedInput: "0", output: "1", reasoning: "0" },
      measurements: "1",
      estimated: "0",
    });
    const rows = historyRows({
      cursor: "1",
      resolution: "day",
      truncated: false,
      series: [
        { model: "a", points: [point("1000", "9007199254740993"), point("2000", "1")] },
        { model: "b", points: [point("1000", "2")] },
      ],
    });
    expect(rows.map((row) => row.startUnixMs)).toEqual(["2000", "1000"]);
    // 64-bit counts stay exact.
    expect(rows[1].tokens.input).toBe("9007199254740995");
    expect(rows[1].measurements).toBe("2");
  });

  it("names Crafts by Harness and checks release sources like the shell", () => {
    expect(craftTitle({ craftId: "x", version: "1", harnesses: [] })).toBe("x");
    expect(
      craftTitle({
        craftId: "x",
        version: "1",
        harnesses: [
          { id: "codex", name: "Codex" },
          { id: "claude", name: "Claude Code" },
        ],
      }),
    ).toBe("Codex and Claude Code");
    expect(releaseProblem("openai/codex", "v1.2.3")).toBeNull();
    expect(releaseProblem("openai", "v1")).not.toBeNull();
    expect(releaseProblem("openai/codex", "v 1")).not.toBeNull();
    expect(receiptText({ kind: "account_unbound", cleanup: "keyring_item_remains" })).toBe(
      "Jet no longer uses this sign-in. Remove it from your system keyring if you don't need it.",
    );
  });
});

describe("agents session", () => {
  function session() {
    const stale = vi.fn();
    const agents = new AgentsSession({ planeStateChanged: () => undefined, planeStateStale: stale });
    agents.select("local");
    return { agents, stale };
  }

  it("drops a stale load after a Plane switch", async () => {
    const pending: Array<(value: AgentsView) => void> = [];
    ipc((command) => {
      if (command === "load_agents") return new Promise<AgentsView>((resolve) => pending.push(resolve));
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    const first = agents.load();
    agents.select(REMOTE);
    const second = agents.load();
    pending[1](view("20"));
    await second;
    pending[0](view("5"));
    await first;
    expect(agents.view).toMatchObject({ kind: "ready", data: { cursor: "20" } });
  });

  it("binds through a confirmed review and reloads", async () => {
    const calls: Array<[string, unknown]> = [];
    ipc((command, args) => {
      calls.push([command, args]);
      if (command === "prepare_account_bind") return bindReview();
      if (command === "apply_settings_change") {
        return { kind: "applied", detail: { kind: "account_bound", bindingId: BINDING } } satisfies SettingsReceipt;
      }
      if (command === "load_agents") return view("30");
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    await agents.prepareBind("openai");
    expect(agents.operation).toMatchObject({ kind: "confirm", review: { reviewId: "r-1" } });
    expect(calls.map(([command]) => command)).toEqual(["prepare_account_bind"]);
    await agents.confirm();
    expect(agents.operation).toMatchObject({ kind: "done", detail: { kind: "account_bound" } });
    expect(calls.map(([command]) => command)).toEqual(["prepare_account_bind", "apply_settings_change", "load_agents"]);
    expect(calls[1][1]).toEqual({ planeId: "local", reviewId: "r-1" });
  });

  it("retries an uncertain change with the same review ID", async () => {
    let applies = 0;
    const sent: unknown[] = [];
    ipc((command, args) => {
      if (command === "prepare_account_unbind") {
        return { ...bindReview("r-9"), subject: { kind: "unbind", bindingId: BINDING, label: "L", provider: "openai", credentialSource: "harness_native" } };
      }
      if (command === "apply_settings_change") {
        sent.push(args);
        if (++applies === 1) throw failure("transport.offline", "offline", true);
        return { kind: "applied", detail: { kind: "account_unbound", cleanup: "none" } };
      }
      if (command === "load_agents") return view();
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    await agents.unbind(BINDING);
    expect(agents.operation).toMatchObject({ kind: "uncertain", reviewId: "r-9" });
    // Nothing else may start meanwhile.
    expect(agents.idle).toBe(false);
    await agents.prepareBind("openai");
    expect(agents.operation.kind).toBe("uncertain");
    await agents.retry();
    expect(agents.operation).toMatchObject({ kind: "done" });
    expect(sent).toEqual([
      { planeId: "local", reviewId: "r-9" },
      { planeId: "local", reviewId: "r-9" },
    ]);
  });

  it("ends on a refusal and refreshes Plane state for a degraded audit", async () => {
    ipc((command) => {
      if (command === "prepare_craft_disable") return { ...bindReview("r-2"), subject: { kind: "disable_craft", craftId: "codex", harnessNames: ["Codex"], mode: "wait" } };
      if (command === "apply_settings_change") return { kind: "refused", error: failure("security.audit_degraded", "conflict") };
      throw new Error(`Unexpected ${command}`);
    });
    const { agents, stale } = session();
    await agents.disableCraft("codex", "wait");
    expect(agents.operation).toMatchObject({ kind: "refused", error: { code: "security.audit_degraded" } });
    expect(stale).toHaveBeenCalledOnce();
    expect(agents.idle).toBe(true);
  });

  it("treats a failed prepare as a refusal: nothing was sent", async () => {
    ipc((command) => {
      if (command === "discover_craft") throw failure("craft.developer_mode_required", "conflict");
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    await agents.discover({ type: "github_release", repository: "a/b", tag: "v1" });
    expect(agents.operationFor({ action: "install" })).toMatchObject({ kind: "refused" });
    expect(agents.operationFor({ action: "bind", provider: "openai" })).toEqual({ kind: "idle" });
  });

  it("keeps an uncertain change across a Plane switch", async () => {
    ipc((command) => {
      if (command === "prepare_account_bind") return bindReview("r-5");
      if (command === "apply_settings_change") throw failure("transport.offline", "offline", true);
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    await agents.prepareBind("openai");
    await agents.confirm();
    agents.select(REMOTE);
    expect(agents.operation).toEqual({ kind: "idle" });
    agents.select("local");
    expect(agents.operation).toMatchObject({ kind: "uncertain", reviewId: "r-5" });
  });

  it("reports account events after the loaded list only", async () => {
    ipc((command) => {
      if (command === "load_agents") return view("10");
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    await agents.load();
    agents.noteChange("account.bound", "10");
    expect(agents.accountsChanged).toBe(false);
    agents.noteChange("account.unbound", "11");
    expect(agents.accountsChanged).toBe(true);
    await agents.load();
    expect(agents.accountsChanged).toBe(false);
  });

  it("marks an open account detail changed only after its own snapshot", async () => {
    let cursor = "10";
    ipc((command) => {
      if (command === "load_account_detail") {
        return { bindingId: BINDING, quotaWindows: [], autoContinue: { policy: { mode: "off" }, retry: null }, cursor, issues: [] };
      }
      throw new Error(`Unexpected ${command}`);
    });
    const { agents } = session();
    await agents.loadDetail(BINDING);
    agents.noteChange("auto_continue.configured", "9");
    expect(agents.changedDetails).toEqual([]);
    agents.noteChange("auto_continue.configured", "12");
    expect(agents.changedDetails).toEqual([BINDING]);
    cursor = "12";
    await agents.loadDetail(BINDING);
    expect(agents.changedDetails).toEqual([]);
    await settle();
  });
});
