import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type {
  ConversationPage,
  ConversationRow,
  ConversationSearchResult,
  PlaneUpdate,
  PublicError,
} from "../src/lib/jet/bridge";
import type { Plane, PlanesSnapshot } from "../src/lib/jet/planes";
import { PlaneCatalog } from "../src/lib/features/planes/catalog.svelte";
import { PlanesSession, type FeedHandler } from "../src/lib/features/planes/session.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  listed = null;
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function failure(category: string, code: string, extra: Partial<PublicError> = {}): PublicError {
  return {
    category,
    code,
    message: `${code} message`,
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
    ...extra,
  };
}

const stale = () =>
  failure("conflict", "query.pagination_stale", {
    retryable: true,
    restart: { reason: "pagination_stale", currentSnapshotRevision: "99" },
  });

function plane(planeId: string, kind: "local" | "remote", label: string): Plane {
  return {
    planeId,
    kind,
    label,
    planeIdentity: null,
    connection: { state: "online" },
    coreVersion: "0.2.0",
    credential: kind === "remote" ? "durable" : null,
    security: "trusted",
    store: "serving",
    features: [],
    protocol: { exact: null, atLeast: 37, atMost: null },
  };
}

function snapshot(planes: Plane[], restoredSelection: PlanesSnapshot["restoredSelection"] = null): PlanesSnapshot {
  return {
    planes,
    identity: { clientId: "00000000-0000-4000-8000-00000000000c", key: "unknown", fingerprint: null },
    restoredSelection,
    notice: null,
    maximumRemotePlanes: 16,
  };
}

const LOCAL_PLANE = plane("local", "local", "This computer");
const REMOTE_PLANE = plane(REMOTE, "remote", "build@host");

function row(planeId: string, id: string, createdAtUnixMs: number): ConversationRow {
  return { planeId, id, revision: "1", title: `Task ${id}`, createdAtUnixMs: String(createdAtUnixMs), projectId: null };
}

/** An oldest-first page chain of `count` rows on one Plane, 256 per page. */
function chain(planeId: string, count: number, prefix = planeId === "local" ? "l" : "r") {
  const rows = Array.from({ length: count }, (_, index) => row(planeId, `${prefix}${String(index).padStart(5, "0")}`, 1_000 + index));
  const pages: ConversationRow[][] = [];
  for (let start = 0; start < rows.length || pages.length === 0; start += 256) {
    pages.push(rows.slice(start, start + 256));
  }
  return (nextPage: string | null): ConversationPage => {
    const index = nextPage === null ? 0 : Number(nextPage.split(":")[1]);
    return {
      planeId,
      cursor: "40",
      conversations: pages[index],
      nextPage: index + 1 < pages.length ? `${planeId}:${index + 1}` : null,
    };
  };
}

type Handler = (command: string, args: Record<string, unknown>) => unknown;

/** What `list_planes` answers unless a test's handler answers it itself. */
let listed: PlanesSnapshot | null = null;

function ipc(handler: Handler) {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  const feeds: Array<{ planeId: string | null; channel: Channel<PlaneUpdate> }> = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const { onUpdate, ...plain } = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: plain });
    if (command === "open_plane_feed") {
      feeds.push({ planeId: plain.planeId as string | null, channel: onUpdate as Channel<PlaneUpdate> });
      return {
        state: "online",
        feedId: `feed-${feeds.length}`,
        planeId: plain.planeId ?? "local",
        planeIdentity: null,
        health: { security: "trusted", store: "serving" },
        coreVersion: "0.2.0",
        daemonStarts: "1",
        startedAtUnixMs: "1",
        cursor: "40",
      };
    }
    if (command === "close_plane_feed") return null;
    if (command === "list_planes" && listed) return listed;
    return handler(command, plain);
  });
  return { calls, feeds };
}

const noFeedHandler: FeedHandler = { receive: () => undefined, opened: () => undefined, openFailed: () => undefined };

function catalogFor(planes: Plane[]) {
  const session = new PlanesSession(noFeedHandler);
  listed = snapshot(planes);
  session.snapshot = listed;
  return { planes: session, catalog: new PlaneCatalog(session) };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

describe("PlaneCatalog Recent", () => {
  it("loads Planes independently: an offline remote Plane leaves local rows ready", async () => {
    const local = chain("local", 3);
    ipc((command, args) => {
      if (command === "list_planes") return snapshot([LOCAL_PLANE, REMOTE_PLANE]);
      if (command !== "load_conversations") throw new Error(`Unexpected ${command}`);
      if (args.planeId === REMOTE) throw failure("offline", "transport.offline");
      return local(args.nextPage as string | null);
    });
    const { catalog } = catalogFor([LOCAL_PLANE, REMOTE_PLANE]);
    catalog.sync();
    await Promise.all([catalog.settled("local"), catalog.settled(REMOTE)]);

    expect(catalog.section("local")?.state.kind).toBe("ready");
    expect(catalog.section(REMOTE)?.state.kind).toBe("offline");
    expect(catalog.rows.map((item) => item.id)).toEqual(["l00002", "l00001", "l00000"]);
    expect(catalog.rows[0].planeLabel).toBe("This computer");
    expect(catalog.statusRows).toMatchObject([
      { planeId: REMOTE, kind: "offline", text: "build@host is offline.", action: "retry" },
    ]);
  });

  it("opens an unfenced feed for a Plane whose first page failed, so it can recover", async () => {
    const { calls } = ipc(() => {
      throw failure("offline", "transport.offline");
    });
    const { catalog } = catalogFor([LOCAL_PLANE]);
    await catalog.load("local");
    expect(catalog.section("local")?.state.kind).toBe("offline");
    expect(calls.filter((call) => call.command === "open_plane_feed")).toEqual([
      { command: "open_plane_feed", args: { planeId: "local", after: null, reset: false } },
    ]);
  });

  it("walks a 20-page chain and shows the newest rows, not the oldest", async () => {
    const local = chain("local", 20 * 256);
    const { calls } = ipc((command, args) => {
      if (command === "list_planes") return snapshot([LOCAL_PLANE]);
      return local(args.nextPage as string | null);
    });
    const { catalog } = catalogFor([LOCAL_PLANE]);
    await catalog.load("local");

    expect(calls.filter((call) => call.command === "load_conversations")).toHaveLength(20);
    expect(catalog.section("local")).toMatchObject({ chain: "complete", pages: 20, tailPage: "local:19", cursor: "40" });
    expect(catalog.rows).toHaveLength(4096);
    expect(catalog.visibleRows).toHaveLength(200);
    expect(catalog.visibleRows[0].id).toBe("l05119");
    expect(catalog.rows.at(-1)?.id).toBe("l01024");
    expect(catalog.hasMore).toBe(true);
    catalog.showMore();
    expect(catalog.visibleRows).toHaveLength(400);
  });

  it("opens each Plane's feed after its first page, fenced at that page's cursor", async () => {
    const local = chain("local", 300);
    const { calls } = ipc((command, args) => local(args.nextPage as string | null));
    const { catalog } = catalogFor([LOCAL_PLANE]);
    await catalog.load("local");
    await catalog.load("local");
    const opens = calls.filter((call) => call.command === "open_plane_feed");
    expect(opens).toEqual([{ command: "open_plane_feed", args: { planeId: "local", after: "40", reset: false } }]);
  });

  it("restarts only that Plane's chain on pagination_stale and marks the third restart partial", async () => {
    const local = chain("local", 600);
    const remote = chain(REMOTE, 2);
    let staleLeft = 1;
    const loads: Array<string | null> = [];
    ipc((command, args) => {
      if (args.planeId === REMOTE) return remote(args.nextPage as string | null);
      loads.push(args.nextPage as string | null);
      if (args.nextPage === "local:2" && staleLeft > 0) {
        staleLeft -= 1;
        throw stale();
      }
      return local(args.nextPage as string | null);
    });
    const { catalog } = catalogFor([LOCAL_PLANE, REMOTE_PLANE]);
    await Promise.all([catalog.load("local"), catalog.load(REMOTE)]);
    expect(loads).toEqual([null, "local:1", "local:2", null, "local:1", "local:2"]);
    expect(catalog.section("local")).toMatchObject({ chain: "complete", restarts: 1 });
    expect(catalog.section(REMOTE)).toMatchObject({ chain: "complete", restarts: 0, pages: 1 });

    staleLeft = 10;
    await catalog.load("local");
    expect(catalog.section("local")).toMatchObject({ chain: "partial", partial: "restarts", restarts: 3 });
    // Rows already shown stay visible.
    expect(catalog.rows.filter((item) => item.planeId === "local")).toHaveLength(600);
    expect(catalog.statusRows).toMatchObject([
      { planeId: "local", kind: "partial", text: "Newest tasks from This computer may be missing." },
    ]);
  });

  it("stops a remote chain after 256 pages with the may-be-missing row", async () => {
    let served = 0;
    ipc((_command, args) => {
      served += 1;
      const index = args.nextPage === null ? 0 : Number(String(args.nextPage).split(":")[1]);
      return {
        planeId: REMOTE,
        cursor: "7",
        conversations: [row(REMOTE, `r${index}`, index)],
        nextPage: `${REMOTE}:${index + 1}`,
      };
    });
    const { catalog } = catalogFor([LOCAL_PLANE, REMOTE_PLANE]);
    await catalog.load(REMOTE);
    expect(served).toBe(256);
    expect(catalog.section(REMOTE)).toMatchObject({ chain: "partial", partial: "page_limit", pages: 256 });
    expect(catalog.statusRows).toMatchObject([
      { planeId: REMOTE, kind: "partial", text: "Newest tasks from build@host may be missing.", action: "open_planes" },
    ]);
  });

  it("re-walks only the Plane whose feed reported conversation.created, debounced", async () => {
    vi.useFakeTimers();
    let localRows = 2;
    const loads: Array<string | null> = [];
    ipc((_command, args) => {
      loads.push(args.planeId as string);
      if (args.planeId === REMOTE) return chain(REMOTE, 1)(args.nextPage as string | null);
      return chain("local", localRows)(args.nextPage as string | null);
    });
    const { catalog } = catalogFor([LOCAL_PLANE, REMOTE_PLANE]);
    await Promise.all([catalog.load("local"), catalog.load(REMOTE)]);
    loads.length = 0;

    localRows = 3;
    catalog.receiveEvent("local", "conversation.created");
    catalog.receiveEvent("local", "conversation.created");
    catalog.receiveEvent(REMOTE, "turn.input");
    expect(loads).toEqual([]);
    await vi.advanceTimersByTimeAsync(1_000);
    await catalog.settled("local");
    expect(loads).toEqual(["local"]);
    expect(catalog.rows[0].id).toBe("l00002");
  });

  it("ignores a late page from a forgotten Plane", async () => {
    const late = deferred<ConversationPage>();
    ipc((_command, args) => (args.planeId === REMOTE ? late.promise : chain("local", 1)(null)));
    const { planes, catalog } = catalogFor([LOCAL_PLANE, REMOTE_PLANE]);
    const walk = catalog.load(REMOTE);
    await catalog.load("local");
    listed = snapshot([LOCAL_PLANE]);
    planes.snapshot = listed;
    catalog.sync();
    late.resolve(chain(REMOTE, 1)(null));
    await walk;
    expect(catalog.section(REMOTE)).toBeNull();
    expect(catalog.rows.map((item) => item.planeId)).toEqual(["local"]);
  });

  it("marks a Plane stale on a feed failure and reloads it on reconnect", async () => {
    const local = chain("local", 1);
    const { calls } = ipc((_command, args) => local(args.nextPage as string | null));
    const { catalog } = catalogFor([LOCAL_PLANE]);
    await catalog.load("local");
    catalog.markUnavailable("local", failure("offline", "transport.offline"));
    expect(catalog.section("local")?.state.kind).toBe("stale");
    expect(catalog.rows).toHaveLength(1);
    expect(catalog.statusRows[0]).toMatchObject({ kind: "stale", action: "retry" });

    calls.length = 0;
    catalog.reconnected("local");
    await catalog.settled("local");
    expect(calls.map((call) => call.command).filter((command) => command !== "list_planes")).toEqual([
      "load_conversations",
    ]);
    expect(catalog.section("local")?.state.kind).toBe("ready");
  });
});

describe("PlaneCatalog Search", () => {
  const result = (planeId: string, ids: string[]): ConversationSearchResult => ({
    planeId,
    cursor: "9",
    indexedThrough: "9",
    hits: ids.map((id, index) => ({ conversationId: id, sequence: String(index), field: "name", excerpt: id })),
  });

  it("groups results by Plane; an unsupported Plane does not hide the others", async () => {
    ipc((command, args) => {
      if (command !== "search_conversations") throw new Error(`Unexpected ${command}`);
      if (args.planeId === REMOTE) {
        throw failure("incompatible", "protocol.feature_unavailable", {
          protocolLimit: { requiredMinor: 12, negotiatedMinor: 9 },
        });
      }
      return result("local", ["b", "a"]);
    });
    const { catalog } = catalogFor([LOCAL_PLANE, REMOTE_PLANE]);
    await catalog.search("fix");
    expect(catalog.groups.map((group) => [group.planeId, group.state.kind, group.text])).toEqual([
      ["local", "ready", null],
      [REMOTE, "unsupported", "Search isn't available on build@host. Update Jet on build@host."],
    ]);
    catalog.toggleSearchFilter(REMOTE);
    expect(catalog.groups.map((group) => group.planeId)).toEqual([REMOTE]);
  });

  it("does not ask a Plane that failed for good", async () => {
    const { calls } = ipc(() => result("local", []));
    const denied = { ...REMOTE_PLANE, connection: { state: "failed" as const, error: failure("unauthorized", "connection.unauthorized") } };
    const { catalog } = catalogFor([LOCAL_PLANE, denied]);
    await catalog.search("fix");
    expect(calls.map((call) => call.args.planeId)).toEqual(["local"]);
    expect(catalog.groups[1]).toMatchObject({ planeId: REMOTE, state: { kind: "failed" } });
  });

  it("discards late results of an earlier search", async () => {
    const first = deferred<ConversationSearchResult>();
    ipc((_command, args) => (args.text === "old" ? first.promise : result("local", ["new"])));
    const { catalog } = catalogFor([LOCAL_PLANE]);
    const earlier = catalog.search("old");
    await catalog.search("new");
    first.resolve(result("local", ["old"]));
    await earlier;
    expect(catalog.searchState?.text).toBe("new");
    const group = catalog.groups[0];
    expect(group.state.kind === "ready" && group.state.result.hits.map((hit) => hit.conversationId)).toEqual(["new"]);
  });
});

describe("DesktopSession with the catalog", () => {
  function desktopIpc(restored: PlanesSnapshot["restoredSelection"], remoteOnline = true) {
    const detailLoads: Array<Record<string, unknown>> = [];
    const harness = ipc((command, args) => {
      switch (command) {
        case "list_planes":
          return snapshot([LOCAL_PLANE, REMOTE_PLANE], restored);
        case "load_setup":
          throw failure("offline", "transport.offline");
        case "load_conversations":
          if (args.planeId === REMOTE) {
            if (!remoteOnline) throw failure("offline", "transport.offline");
            return chain(REMOTE, 2)(args.nextPage as string | null);
          }
          return chain("local", 3)(args.nextPage as string | null);
        case "load_conversation": {
          detailLoads.push(args);
          return {
            conversation: row(args.planeId as string, args.conversationId as string, 1),
            cursor: "40",
            workspaceId: null,
            workspaceRoot: null,
            runs: [],
          };
        }
        case "load_run_supervision":
          return { cursor: "40", maximumEntries: 128, maximumPromptBytes: 65536, turns: [], execution: null };
        default:
          throw new Error(`Unexpected ${command}`);
      }
    });
    return { ...harness, detailLoads };
  }

  async function settle(session: DesktopSession) {
    for (let round = 0; round < 20; round += 1) await Promise.resolve();
    await Promise.all([session.catalog.settled("local"), session.catalog.settled(REMOTE)]);
    for (let round = 0; round < 20; round += 1) await Promise.resolve();
  }

  it("opens the restored selection on its own Plane once that Plane is ready", async () => {
    const { detailLoads } = desktopIpc({ planeId: REMOTE, conversationId: "r00001" });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.selection).toEqual({ planeId: REMOTE, conversationId: "r00001" });
    expect(detailLoads).toEqual([{ conversationId: "r00001", planeId: REMOTE }]);
    expect(session.runsOnLabel).toBe("build@host");
  });

  it("keeps a restored selection on an unavailable Plane without loading fixture content", async () => {
    const { detailLoads } = desktopIpc({ planeId: REMOTE, conversationId: "r00001" }, false);
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.selection).toEqual({ planeId: REMOTE, conversationId: "r00001" });
    expect(session.selectionUnavailable).toBe(true);
    expect(detailLoads).toEqual([]);
    expect(session.catalog.section("local")?.state.kind).toBe("ready");
  });

  it("selects the newest task when nothing was restored", async () => {
    desktopIpc(null);
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.selection).toEqual({ planeId: "local", conversationId: "l00002" });
  });

  it("an event on Plane B refreshes neither Plane A's Recent nor its timeline", async () => {
    vi.useFakeTimers();
    const { calls, feeds } = desktopIpc(null);
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    const remoteFeed = feeds.find((feed) => feed.planeId === REMOTE);
    expect(remoteFeed).toBeDefined();
    calls.length = 0;
    remoteFeed?.channel.onmessage({
      type: "event",
      sequence: "41",
      recorded_at_unix_ms: "1",
      kind: "conversation.created",
      conversation_id: "l00002",
      run_id: null,
      timeline: [{ kind: "user", text: "remote", itemId: "t", approval: null }],
    });
    await vi.advanceTimersByTimeAsync(1_000);
    await settle(session);
    expect(session.timeline).toEqual([]);
    expect(
      calls.filter((call) => call.command === "load_conversations").map((call) => call.args.planeId),
    ).toEqual([REMOTE]);
  });

  it("opens Planes on the requested Plane without the placeholder notice", () => {
    desktopIpc(null);
    const session = new DesktopSession();
    session.planes.snapshot = snapshot([LOCAL_PLANE, REMOTE_PLANE]);
    session.openPlanes({ planeId: REMOTE, focus: "detail" });
    expect(session.sidebarSelection).toBe("planes");
    expect(session.actionNotice).toBeNull();
    expect(session.planes.selectedPlaneId).toBe(REMOTE);
    expect(session.planes.focusRequest?.section).toBe("detail");
  });
});
