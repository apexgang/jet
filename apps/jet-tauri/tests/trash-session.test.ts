import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { ConnectionSnapshot, PublicError } from "../src/lib/jet/bridge";
import type { TrashEntry, TrashView } from "../src/lib/jet/retention";
import { TrashSession, type TrashHost } from "../src/lib/features/trash/session.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";
const NOW = 1_800_000_000_000;

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

type Handler = (command: string, args: Record<string, unknown>) => unknown;

function ipc(handler: Handler) {
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC((command, args) => handler(command, (args ?? {}) as Record<string, unknown>));
}

function failure(code: string, category: string, extra: Partial<PublicError> = {}): PublicError {
  return {
    category,
    code,
    message: "Refused.",
    retryable: category === "offline",
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
    ...extra,
  };
}

function entry(id: string, overrides: Partial<TrashEntry> = {}): TrashEntry {
  return {
    conversationId: id,
    reason: "manual_forget",
    trashedAtUnixMs: String(NOW - 1000),
    expiresAtUnixMs: String(NOW + 86_400_000),
    restorable: true,
    ...overrides,
  };
}

function view(planeId: string, entries: TrashEntry[], capped = false): TrashView {
  return { planeId, planeLabel: planeId === "local" ? "This computer" : "Build box", cursor: "7", graceDays: 30, capped, entries };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function host(known: Record<string, string> = {}, outcomes: Array<[string, string | null]> = []): TrashHost {
  return {
    planeIds: () => ["local", REMOTE],
    knownTitle: (planeId, id) => known[`${planeId}/${id}`] ?? null,
    observe: (planeId, error) => outcomes.push([planeId, error?.code ?? null]),
  };
}

async function settle() {
  for (let index = 0; index < 5; index += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("TrashSession sections", () => {
  it("keeps one section per Plane: one offline while another is ready", async () => {
    ipc((command, args) => {
      if (command === "load_trash" && args.planeId === "local") throw failure("transport.offline", "offline");
      if (command === "load_trash") return view(REMOTE, [entry("c1")]);
      if (command === "resolve_conversation_names") return [{ conversationId: "c1", title: "Fix login" }];
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host(), () => NOW);
    trash.show();
    await settle();
    expect(trash.section("local")).toEqual({ kind: "offline", last: null });
    expect(trash.section(REMOTE).kind).toBe("ready");
    expect(trash.title(REMOTE, "c1")).toBe("Fix login");
  });

  it("classifies empty, unsupported and denied, and keeps the capped flag", async () => {
    const answers: unknown[] = [
      view("local", []),
      failure("protocol.feature_unavailable", "incompatible", { protocolLimit: { requiredMinor: 39, negotiatedMinor: 30 } }),
      failure("unauthorized", "unauthorized"),
      view("local", Array.from({ length: 256 }, (_, index) => entry(`c${index}`)), true),
    ];
    ipc((command) => {
      if (command === "resolve_conversation_names") return [];
      const answer = answers.shift();
      if (answer && typeof answer === "object" && "category" in answer) throw answer;
      return answer;
    });
    const trash = new TrashSession(host(), () => NOW);
    await trash.load("local");
    expect(trash.section("local").kind).toBe("empty");
    await trash.load("local");
    expect(trash.section("local").kind).toBe("unsupported");
    await trash.load("local");
    expect(trash.section("local").kind).toBe("denied");
    await trash.load("local");
    const section = trash.section("local");
    expect(section.kind === "ready" && section.view.capped).toBe(true);
  });

  it("ignores a list that arrives after the Plane's generation moved on", async () => {
    const late = deferred<TrashView>();
    ipc((command) => {
      if (command === "load_trash") return late.promise;
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host(), () => NOW);
    const pending = trash.load(REMOTE);
    trash.reset(REMOTE);
    late.resolve(view(REMOTE, [entry("c1")]));
    await pending;
    expect(trash.sections[REMOTE]).toBeUndefined();
  });

  it("reloads a stale section only while Jet Trash is shown", async () => {
    let loads = 0;
    ipc((command) => {
      if (command === "load_trash") {
        loads += 1;
        return view("local", []);
      }
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host(), () => NOW);
    await trash.load("local");
    trash.markStale("local");
    await settle();
    expect(loads).toBe(1);
    expect(trash.section("local")).toMatchObject({ kind: "empty", freshness: "stale" });

    trash.visible = true;
    trash.markStale("local");
    await settle();
    expect(loads).toBe(2);
    expect(trash.section("local")).toMatchObject({ kind: "empty", freshness: "live" });
  });

  it("a restarted daemon drops only that Plane's section and names", async () => {
    ipc((command, args) => {
      if (command === "load_trash") return view(args.planeId as string, [entry("c1")]);
      if (command === "resolve_conversation_names") return [{ conversationId: "c1", title: `Task on ${String(args.planeId)}` }];
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host(), () => NOW);
    await trash.load("local");
    await trash.load(REMOTE);
    trash.reset("local");
    expect(trash.sections.local).toBeUndefined();
    expect(trash.title("local", "c1")).toBeNull();
    expect(trash.section(REMOTE).kind).toBe("ready");
    expect(trash.title(REMOTE, "c1")).toBe(`Task on ${REMOTE}`);
  });

  it("asks the Plane only for names the loaded task list lacks, in one call", async () => {
    const asked: unknown[] = [];
    ipc((command, args) => {
      if (command === "load_trash") return view("local", [entry("known"), entry("a"), entry("b")]);
      if (command === "resolve_conversation_names") {
        asked.push(args.conversationIds);
        return [{ conversationId: "a", title: "Alpha" }, { conversationId: "b", title: null }];
      }
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host({ "local/known": "From Recent" }), () => NOW);
    await trash.load("local");
    expect(asked).toEqual([["a", "b"]]);
    expect(trash.title("local", "known")).toBe("From Recent");
    expect(trash.title("local", "a")).toBe("Alpha");
    expect(trash.title("local", "b")).toBeNull();
    // Names already looked up are not asked for again.
    await trash.load("local");
    expect(asked).toHaveLength(1);
  });
});

describe("TrashSession restore", () => {
  it("removes a restored row only when the reloaded list drops it", async () => {
    const reload = deferred<TrashView>();
    let loads = 0;
    const outcomes: Array<[string, string | null]> = [];
    ipc((command) => {
      if (command === "load_trash") return ++loads === 1 ? view("local", [entry("known")]) : reload.promise;
      if (command === "restore_conversation") return { kind: "restored", conversationId: "known" };
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host({ "local/known": "Known" }, outcomes), () => NOW);
    await trash.load("local");
    const restoring = trash.restore("local", "known");
    await vi.waitFor(() => expect(trash.rowAction("local", "known").kind).toBe("restored"));
    // Not optimistic: the row stays until the Plane's list says otherwise.
    const section = trash.section("local");
    expect(section.kind === "ready" && section.view.entries.map((row) => row.conversationId)).toEqual(["known"]);
    reload.resolve(view("local", []));
    await restoring;
    expect(trash.section("local").kind).toBe("empty");
    expect(trash.rowAction("local", "known")).toEqual({ kind: "idle" });
    expect(outcomes).toEqual([["local", null]]);
  });

  it("keeps the row on a refusal, and offers Try again with the same request when uncertain", async () => {
    const sent: unknown[] = [];
    const answers: unknown[] = [
      { kind: "refused", error: failure("security.audit_degraded", "conflict") },
      failure("transport.offline", "offline"),
      { kind: "restored", conversationId: "c1" },
    ];
    ipc((command, args) => {
      if (command === "load_trash") return view("local", [entry("c1")]);
      if (command === "resolve_conversation_names") return [];
      if (command === "restore_conversation") {
        sent.push(args);
        const answer = answers.shift();
        if (answer && typeof answer === "object" && "category" in answer) throw answer;
        return answer;
      }
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host(), () => NOW);
    await trash.load("local");
    await trash.restore("local", "c1");
    expect(trash.rowAction("local", "c1")).toMatchObject({ kind: "refused", error: { code: "security.audit_degraded" } });
    expect(trash.section("local").kind).toBe("ready");

    await trash.restore("local", "c1");
    expect(trash.rowAction("local", "c1").kind).toBe("uncertain");
    await trash.restore("local", "c1");
    expect(sent).toEqual([
      { planeId: "local", conversationId: "c1" },
      { planeId: "local", conversationId: "c1" },
      { planeId: "local", conversationId: "c1" },
    ]);
  });
});

describe("TrashSession banner", () => {
  it("comes from the task's own Trash status, not list membership", async () => {
    let trashed: TrashEntry | null = null;
    ipc((command) => {
      if (command === "load_trash") return view("local", [entry("c1")]);
      if (command === "resolve_conversation_names") return [];
      if (command === "load_trash_status") return { trash: trashed };
      throw new Error(`Unexpected ${command}`);
    });
    const trash = new TrashSession(host(), () => NOW);
    await trash.load("local");
    await trash.loadBanner("local", "c1");
    expect(trash.bannerFor("local", "c1")).toEqual({ kind: "none" });

    trashed = entry("c2");
    await trash.loadBanner("local", "c2");
    expect(trash.bannerFor("local", "c2")).toEqual({ kind: "trashed", entry: entry("c2") });
    // Another task, or the same UUID on another Plane, shows nothing.
    expect(trash.bannerFor(REMOTE, "c2")).toEqual({ kind: "none" });
    expect(trash.bannerFor("local", "c1")).toEqual({ kind: "none" });
  });
});

describe("DesktopSession with Jet Trash", () => {
  function connection(daemonStarts: string): ConnectionSnapshot {
    return {
      state: "online",
      feedId: "feed",
      planeId: "local",
      planeIdentity: null,
      health: { security: "trusted", store: "serving", ledger: "verified" },
      coreVersion: "0.2.0",
      daemonStarts,
      startedAtUnixMs: "1",
      cursor: "4",
    };
  }

  function desktop(extra: Handler = () => null) {
    ipc((command, args) => {
      if (command === "list_planes") throw failure("transport.offline", "offline");
      return extra(command, args);
    });
    return new DesktopSession();
  }

  it("a new daemon start on one Plane drops only that Plane's Jet Trash", () => {
    const session = desktop();
    session.receive("local", { type: "connected", connection: connection("1") });
    session.trash.sections = {
      local: { kind: "empty", view: view("local", []), freshness: "live", loadedAt: NOW },
      [REMOTE]: { kind: "empty", view: view(REMOTE, []), freshness: "live", loadedAt: NOW },
    };
    session.receive("local", { type: "connected", connection: connection("2") });
    expect(session.trash.sections.local).toBeUndefined();
    expect(session.trash.sections[REMOTE]?.kind).toBe("empty");
  });

  it("restores and trashes mark that Plane's section stale; a dropped feed marks it offline", () => {
    const session = desktop();
    session.trash.sections = {
      local: { kind: "empty", view: view("local", []), freshness: "live", loadedAt: NOW },
      [REMOTE]: { kind: "empty", view: view(REMOTE, []), freshness: "live", loadedAt: NOW },
    };
    const event = (kind: string) => ({
      type: "event" as const,
      sequence: "5",
      recorded_at_unix_ms: "5",
      kind,
      conversation_id: "c1",
      run_id: null,
      timeline: [],
    });
    session.receive(REMOTE, event("conversation.restored"));
    expect(session.trash.sections[REMOTE]).toMatchObject({ freshness: "stale" });
    expect(session.trash.sections.local).toMatchObject({ freshness: "live" });
    session.receive("local", event("setting.changed"));
    expect(session.trash.sections.local).toMatchObject({ freshness: "stale" });
    session.receive(REMOTE, { type: "reconnecting", error: failure("transport.offline", "offline", { planeId: REMOTE }) });
    expect(session.trash.sections[REMOTE]?.kind).toBe("offline");
  });

  it("reads a dropped Plane's Jet Trash again when its feed resumes", async () => {
    const loads: string[] = [];
    const session = desktop((command, args) => {
      if (command !== "load_trash") return null;
      loads.push(String(args.planeId));
      return view(String(args.planeId), []);
    });
    session.trash.sections = {
      local: { kind: "empty", view: view("local", []), freshness: "live", loadedAt: NOW },
      [REMOTE]: { kind: "empty", view: view(REMOTE, []), freshness: "live", loadedAt: NOW },
    };
    // A feed that opens says resumed: nothing to read again.
    session.receive(REMOTE, { type: "resumed", after: "4" });
    await settle();
    expect(loads).toEqual([]);

    // After a drop the feed dials again and says resumed, not connected.
    session.receive(REMOTE, { type: "reconnecting", error: failure("transport.offline", "offline", { planeId: REMOTE }) });
    expect(session.trash.sections[REMOTE]?.kind).toBe("offline");
    session.receive(REMOTE, { type: "resumed", after: "4" });
    await settle();
    expect(loads).toEqual([REMOTE]);
    expect(session.trash.sections[REMOTE]?.kind).toBe("empty");
    expect(session.trash.sections.local).toMatchObject({ kind: "empty", freshness: "live" });

    session.receive(REMOTE, { type: "resumed", after: "4" });
    await settle();
    expect(loads).toEqual([REMOTE]);
  });

  it("opens Jet Trash from the sidebar and hides it when leaving", () => {
    const session = desktop((command) => (command === "load_trash" ? view("local", []) : null));
    session.select("trash");
    expect(session.sidebarSelection).toBe("trash");
    expect(session.trash.visible).toBe(true);
    expect(session.workPanelPresented).toBe(false);
    session.select("conversation");
    expect(session.trash.visible).toBe(false);
  });

  it("offers Move to Trash only for a selected task on an online Plane", () => {
    const session = desktop();
    expect(session.canMoveToTrash).toBe(false);
    session.selectedConversationId = "c1";
    expect(session.canMoveToTrash).toBe(false);
    session.receive("local", { type: "connected", connection: connection("1") });
    expect(session.canMoveToTrash).toBe(true);
  });
});
