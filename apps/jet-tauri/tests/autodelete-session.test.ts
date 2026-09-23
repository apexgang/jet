import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import {
  byteCounter,
  candidatesHeading,
  changeRefusalText,
  draftingText,
  parseDays,
  promptBytes,
  promptFits,
  refusalCopy,
  POLL_DELAY_MS,
} from "../src/lib/features/autodelete/model";
import { AutodeleteSession } from "../src/lib/features/autodelete/session.svelte";
import { SystemSession } from "../src/lib/features/system/session.svelte";
import type { PublicError } from "../src/lib/jet/bridge";
import type { AutodeleteRule, AutodeleteRules, RuleState } from "../src/lib/jet/retention";

const REMOTE = "0000000a-0000-4000-8000-000000000002";
const RULE = "00000000-0000-0000-0000-000000000ad2";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function failure(code: string, category = "conflict", planeId: string | null = "local"): PublicError {
  return {
    category,
    code,
    message: "Message",
    retryable: category === "offline",
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId,
  };
}

function rule(state: RuleState, overrides: Partial<AutodeleteRule> = {}): AutodeleteRule {
  return {
    ruleId: RULE,
    prompt: "Old scratch tasks",
    state,
    scope: "forget",
    createdAtUnixMs: "1700000000000",
    updatedAtUnixMs: "1700000000000",
    candidates: [],
    candidatesCapped: false,
    attribution: null,
    ...overrides,
  };
}

function view(rules: AutodeleteRule[], enabled: boolean | null = false, pending: AutodeleteRules["pending"] = []): AutodeleteRules {
  return {
    planeId: "local",
    planeLabel: "This computer",
    cursor: "7",
    drafting: { enabled, bindingConfigured: false },
    rules,
    pending,
  };
}

type Call = { command: string; args: Record<string, unknown> };

/** A fake shell answering each command through `handlers`. */
function install(handlers: Record<string, (args: Record<string, unknown>) => unknown>): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC((command, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    calls.push({ command, args });
    const handler = handlers[command];
    if (!handler) throw failure("client.state_unavailable", "internal");
    return handler(args);
  });
  return calls;
}

async function flush(): Promise<void> {
  for (let turn = 0; turn < 12; turn++) await Promise.resolve();
}

function loads(calls: Call[]): number {
  return calls.filter((call) => call.command === "load_autodelete_rules").length;
}

describe("auto-delete model", () => {
  it("counts prompt bytes, not characters, against the 4,096-byte bound", () => {
    expect(promptBytes("€")).toBe(3);
    expect(byteCounter("a".repeat(4096))).toBe("4,096 / 4,096 bytes");
    expect(promptFits("")).toBe(false);
    expect(promptFits("a")).toBe(true);
    expect(promptFits("a".repeat(4096))).toBe(true);
    expect(promptFits("a".repeat(4097))).toBe(false);
    expect(promptFits("€".repeat(1366))).toBe(false);
    expect(promptFits("line\n\tnext")).toBe(true);
    expect(promptFits("bell\u0007")).toBe(false);
  });

  it("accepts whole days from 1 to 36,500", () => {
    expect(parseDays("30")).toBe(30);
    expect(parseDays(" 1 ")).toBe(1);
    expect(parseDays("36500")).toBe(36500);
    for (const invalid of ["0", "36501", "2.5", "-3", "", "1e3"]) expect(parseDays(invalid)).toBeNull();
  });

  it("discloses drafting for on, off and unknown", () => {
    expect(draftingText(true)).toBe(
      "Jet sends this rule's text to the Utility Provider selected in Settings › Agents to draft it. Task contents aren't sent.",
    );
    expect(draftingText(false)).toBe(
      "Rule drafting is off. Jet records the rule as not drafted, and you set the number of days yourself.",
    );
    expect(draftingText(null)).toBe(
      "Jet couldn't check whether rule drafting is on. If it's off, you set the number of days yourself.",
    );
    expect(refusalCopy("utility.disabled")).toEqual({
      text: "Rule drafting is off. Turn it on in Settings › Agents, or set the number of days yourself.",
      drafting: true,
    });
    expect(refusalCopy("utility.model_refused").drafting).toBe(false);
    expect(changeRefusalText(failure("autodelete.interpretation_changed"))).toBe(
      "This rule changed on another device. Review the new version before approving.",
    );
    expect(changeRefusalText(failure("autodelete.retry_mismatch"))).toBe(
      "Jet is still confirming your previous change to this rule. Try that one again first.",
    );
  });

  it("notes when the Plane listed only its first 32 candidates", () => {
    const candidate = { conversationId: "c1", lastActiveAtUnixMs: "1", protections: [] };
    expect(candidatesHeading({ candidates: [candidate], candidatesCapped: false })).toBe("1 task matches today");
    expect(candidatesHeading({ candidates: Array(32).fill(candidate), candidatesCapped: true })).toBe(
      "32 tasks match today (showing the first 32)",
    );
  });
});

describe("auto-delete session", () => {
  let rules: AutodeleteRules;

  beforeEach(() => {
    rules = view([]);
  });

  it("creates a new rule with no rule ID, then shows the drafting-off refusal and drafts by hand", async () => {
    const calls = install({
      load_autodelete_rules: () => rules,
      change_autodelete_rule: (args) => {
        const change = args.change as { kind: string; inactive_days?: number };
        if (change.kind === "compile") {
          // Drafting is off: the Plane records the rule, then refuses it.
          rules = view([rule({ kind: "refused", reason: "utility.disabled" })]);
          return { kind: "recorded", rule: rule({ kind: "compiling" }) };
        }
        rules = view([rule({ kind: "draft", inactiveDays: change.inactive_days!, approveToken: "token-1" })]);
        return { kind: "recorded", rule: rules.rules[0] };
      },
    });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    expect(session.rules.kind).toBe("ready");

    session.compose = "Old scratch tasks";
    await session.createDraft();
    expect(calls[1]).toEqual({
      command: "change_autodelete_rule",
      args: { planeId: "local", change: { kind: "compile", rule_id: null, prompt: "Old scratch tasks" } },
    });
    expect(session.compose).toBe("");
    const refused = session.rule(RULE)!;
    expect(refused.state).toEqual({ kind: "refused", reason: "utility.disabled" });
    expect(refusalCopy((refused.state as { reason: string }).reason).drafting).toBe(true);

    session.editDays(RULE);
    expect(session.editor).toEqual({ kind: "days", ruleId: RULE, days: "" });
    session.setEditorText("30");
    await session.saveEdit();
    expect(calls.at(-2)).toEqual({
      command: "change_autodelete_rule",
      args: { planeId: "local", change: { kind: "set_inactive_days", rule_id: RULE, inactive_days: 30 } },
    });
    expect(session.rule(RULE)!.state).toEqual({ kind: "draft", inactiveDays: 30, approveToken: "token-1" });
    expect(session.editor).toEqual({ kind: "idle" });
  });

  it("reads a compiling rule again every 2 s and stops after five reads", async () => {
    vi.useFakeTimers();
    rules = view([rule({ kind: "compiling" })]);
    const calls = install({ load_autodelete_rules: () => rules });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    expect(loads(calls)).toBe(1);

    for (let poll = 1; poll <= 5; poll++) {
      await vi.advanceTimersByTimeAsync(POLL_DELAY_MS);
      await flush();
      expect(loads(calls)).toBe(1 + poll);
    }
    expect(session.stillPreparing).toBe(true);
    await vi.advanceTimersByTimeAsync(POLL_DELAY_MS * 5);
    expect(loads(calls)).toBe(6);

    // Refresh starts waiting again; a draft ends the polling.
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "t" })]);
    await session.refresh();
    expect(session.stillPreparing).toBe(false);
    await vi.advanceTimersByTimeAsync(POLL_DELAY_MS * 3);
    expect(loads(calls)).toBe(7);
  });

  it("a Plane switch stops polling and drops a late read", async () => {
    vi.useFakeTimers();
    rules = view([rule({ kind: "compiling" })]);
    let release: (() => void) | null = null;
    const calls = install({
      load_autodelete_rules: (args) =>
        args.planeId === REMOTE
          ? new Promise((resolve) => {
              release = () => resolve(view([]));
            })
          : rules,
    });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    session.select(REMOTE);
    const loading = session.ensureLoaded();
    await vi.advanceTimersByTimeAsync(POLL_DELAY_MS * 2);
    expect(calls.filter((call) => call.args.planeId === "local")).toHaveLength(1);
    session.select("local");
    release!();
    await loading;
    expect(session.rules.kind).toBe("loading");
  });

  it("approves with only the token, and an open Approve dialog survives a poll", async () => {
    vi.useFakeTimers();
    const draft = rule({ kind: "draft", inactiveDays: 30, approveToken: "token-1" });
    const other = rule({ kind: "compiling" }, { ruleId: "00000000-0000-0000-0000-000000000ad9" });
    rules = view([draft, other]);
    const calls = install({
      load_autodelete_rules: () => rules,
      change_autodelete_rule: () => {
        rules = view([rule({ kind: "approved", inactiveDays: 30, approvedAtUnixMs: "9", everywhereToken: "e-1" }), other]);
        return { kind: "recorded", rule: rules.rules[0] };
      },
    });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    session.openApprove(RULE);
    expect(session.dialog).toEqual({ kind: "approve", ruleId: RULE, token: "token-1", days: 30 });

    // The other rule's compile poll reads the same draft: the dialog stays.
    await vi.advanceTimersByTimeAsync(POLL_DELAY_MS);
    await flush();
    expect(loads(calls)).toBe(2);
    expect(session.dialog.kind).toBe("approve");
    expect(session.block).toBeNull();

    await session.confirmDialog();
    const change = calls.find((call) => call.command === "change_autodelete_rule")!;
    expect(change.args).toEqual({ planeId: "local", change: { kind: "approve", token_id: "token-1" } });
    expect(session.dialog.kind).toBe("closed");
    expect(session.rule(RULE)!.state.kind).toBe("approved");
  });

  it("closes an Approve dialog whose draft changed elsewhere", async () => {
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "token-1" })]);
    install({ load_autodelete_rules: () => rules });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    session.openApprove(RULE);
    rules = view([rule({ kind: "draft", inactiveDays: 45, approveToken: "token-2" })]);
    await session.refresh();
    expect(session.dialog).toEqual({ kind: "changed", ruleId: RULE });
  });

  it("an interpretation changed refusal is explained and reloads the rules", async () => {
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "token-1" })]);
    const calls = install({
      load_autodelete_rules: () => rules,
      change_autodelete_rule: () => ({ kind: "refused", error: failure("autodelete.interpretation_changed") }),
    });
    const observed: string[] = [];
    const session = new AutodeleteSession({ observe: (error) => observed.push(error.code) });
    session.select("local");
    await session.ensureLoaded();
    session.openApprove(RULE);
    await session.confirmDialog();
    await flush();
    expect(session.editor).toEqual({
      kind: "error",
      slot: RULE,
      error: failure("autodelete.interpretation_changed"),
    });
    expect(loads(calls)).toBe(2);
    expect(observed).toEqual(["autodelete.interpretation_changed"]);
  });

  it("an unconfirmed change blocks the rule until Try again resends it unchanged", async () => {
    rules = view([rule({ kind: "draft", inactiveDays: 14, approveToken: "token-1" })]);
    let fail = true;
    const calls = install({
      load_autodelete_rules: () => rules,
      change_autodelete_rule: () => {
        if (fail) throw failure("transport.offline", "offline");
        rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "token-2" })]);
        return { kind: "recorded", rule: rules.rules[0] };
      },
    });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    session.editDays(RULE);
    session.setEditorText("30");
    await session.saveEdit();
    expect(session.pending[RULE]).toEqual({ kind: "set_inactive_days", rule_id: RULE, inactive_days: 30 });
    expect(session.canChange(RULE)).toBe(false);
    // Other edits of this rule wait.
    session.editDays(RULE);
    expect(session.editor.kind).toBe("idle");
    session.openDelete(RULE);
    expect(session.dialog.kind).toBe("closed");

    fail = false;
    await session.retry(RULE);
    const sent = calls.filter((call) => call.command === "change_autodelete_rule").map((call) => call.args.change);
    expect(sent).toEqual([
      { kind: "set_inactive_days", rule_id: RULE, inactive_days: 30 },
      { kind: "set_inactive_days", rule_id: RULE, inactive_days: 30 },
    ]);
    expect(session.pending).toEqual({});
    expect(session.canChange(RULE)).toBe(true);
  });

  it("a retry mismatch shows the change the shell still holds, with Try again", async () => {
    const held = { kind: "set_inactive_days" as const, rule_id: RULE, inactive_days: 30 };
    rules = view([rule({ kind: "draft", inactiveDays: 14, approveToken: "t" })], false, [{ ruleId: RULE, change: held }]);
    install({ load_autodelete_rules: () => rules });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    // Reopened Settings: the unconfirmed change comes from the shell.
    expect(session.pending).toEqual({ [RULE]: held });
    expect(session.canChange(RULE)).toBe(false);
  });

  it("deleting needs the confirmation dialog", async () => {
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "t" })]);
    const calls = install({
      load_autodelete_rules: () => rules,
      change_autodelete_rule: () => {
        rules = view([]);
        return { kind: "deleted", ruleId: RULE };
      },
    });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    await session.confirmDialog();
    expect(calls.some((call) => call.command === "change_autodelete_rule")).toBe(false);
    session.openDelete(RULE);
    await session.confirmDialog();
    expect(calls.find((call) => call.command === "change_autodelete_rule")!.args).toEqual({
      planeId: "local",
      change: { kind: "delete", rule_id: RULE },
    });
    expect(session.rule(RULE)).toBeNull();
  });

  it("names candidates in native calls of at most 32, once per rule", async () => {
    const candidates = Array.from({ length: 40 }, (_, index) => ({
      conversationId: `c${index}`,
      lastActiveAtUnixMs: "1",
      protections: [],
    }));
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "t" }, { candidates })]);
    const calls = install({
      load_autodelete_rules: () => rules,
      resolve_conversation_names: (args) =>
        (args.conversationIds as string[]).map((id) => ({ conversationId: id, title: id === "c0" ? null : `Task ${id}` })),
    });
    const session = new AutodeleteSession();
    session.select(REMOTE);
    await session.ensureLoaded();
    await session.resolveNames(RULE);
    await session.resolveNames(RULE);
    const lookups = calls.filter((call) => call.command === "resolve_conversation_names");
    expect(lookups.map((call) => (call.args.conversationIds as string[]).length)).toEqual([32, 8]);
    expect(lookups[0].args.planeId).toBe(REMOTE);
    expect(session.names.c0).toBeNull();
    expect(session.names.c39).toBe("Task c39");
  });

  it("changes wait while the Plane is read-only or the view is stale", async () => {
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "t" })]);
    let block: "read_only" | "stale" | null = "read_only";
    install({ load_autodelete_rules: () => rules });
    const session = new AutodeleteSession({ mutationBlock: () => block });
    session.select("local");
    await session.ensureLoaded();
    session.openApprove(RULE);
    expect(session.dialog.kind).toBe("closed");
    block = null;
    session.markStale();
    expect(session.block).toBe("stale");
    await session.refresh();
    expect(session.block).toBeNull();
  });

  it("a new Jet service start drops everything read and reads again", async () => {
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "t" })]);
    const calls = install({ load_autodelete_rules: () => rules });
    const session = new AutodeleteSession();
    session.select("local");
    await session.ensureLoaded();
    session.openApprove(RULE);
    session.names = { c1: "Task" };
    session.planeRestarted();
    expect(session.dialog.kind).toBe("closed");
    expect(session.names).toEqual({});
    await flush();
    expect(loads(calls)).toBe(2);
    expect(session.rules.kind).toBe("ready");
  });

  it("a new Jet service start seen by the health read reaches the rules", async () => {
    let starts = "3";
    rules = view([rule({ kind: "draft", inactiveDays: 30, approveToken: "t" })]);
    const calls = install({
      load_autodelete_rules: () => rules,
      load_system_health: () => ({ service: { daemonStarts: starts }, issues: [] }),
    });
    const autodelete = new AutodeleteSession();
    const system = new SystemSession(Date.now, () => autodelete.planeRestarted());
    autodelete.select("local");
    system.select("local");
    await autodelete.ensureLoaded();
    await system.ensureLoaded();
    expect(loads(calls)).toBe(1);
    starts = "4";
    await system.load();
    await flush();
    expect(loads(calls)).toBe(2);
  });
});
