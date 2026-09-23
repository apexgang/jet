import { describe, expect, it } from "vitest";

import type { ConversationRow, ConversationSearchResult, PublicError } from "../src/lib/jet/bridge";
import type { FeatureSupport, Plane } from "../src/lib/jet/planes";
import {
  aggregateStatus,
  classifySection,
  compareDecimal,
  featureLabel,
  identityPrefix,
  mergeRecent,
  planeAttentionCount,
  planeErrorCopy,
  planeStatus,
  recentStatusRows,
  retainNewest,
  searchGroups,
  type CatalogSection,
  type Section,
} from "../src/lib/features/planes/model";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

function error(category: string, code: string, retryable = false): PublicError {
  return {
    category,
    code,
    message: `${code} message`,
    retryable,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

function row(planeId: string, id: string, createdAtUnixMs: string): ConversationRow {
  return { planeId, id, revision: "1", title: `Task ${id}`, createdAtUnixMs, projectId: null };
}

function section(
  planeId: string,
  state: Section<ConversationRow[]>,
  extra: Partial<CatalogSection> = {},
): CatalogSection {
  return {
    planeId,
    planeIdentity: null,
    state,
    cursor: "10",
    tailPage: null,
    pages: 1,
    chain: "complete",
    partial: null,
    restarts: 0,
    savedAtUnixMs: null,
    ...extra,
  };
}

function plane(overrides: Partial<Plane> = {}): Plane {
  return {
    planeId: "local",
    kind: "local",
    label: "This computer",
    planeIdentity: null,
    connection: { state: "online" },
    coreVersion: "0.2.0",
    credential: null,
    security: "trusted",
    store: "serving",
    features: [],
    protocol: { exact: null, atLeast: 0, atMost: null },
    ...overrides,
  };
}

describe("Recent merge", () => {
  it("orders by timestamp descending across Planes", () => {
    const merged = mergeRecent([
      section("local", { kind: "ready", data: [row("local", "a", "100"), row("local", "b", "300")] }),
      section(REMOTE, { kind: "ready", data: [row(REMOTE, "c", "200")] }),
    ]);
    expect(merged.map((item) => item.id)).toEqual(["b", "c", "a"]);
  });

  it("breaks equal timestamps by Plane identity, then Conversation ID", () => {
    const merged = mergeRecent([
      section("local", { kind: "ready", data: [row("local", "z", "5"), row("local", "y", "5")] }, {
        planeIdentity: "bbbbbbbb-0000-4000-8000-000000000000",
      }),
      section(REMOTE, { kind: "ready", data: [row(REMOTE, "x", "5")] }, {
        planeIdentity: "aaaaaaaa-0000-4000-8000-000000000000",
      }),
    ]);
    expect(merged.map((item) => item.id)).toEqual(["x", "y", "z"]);
  });

  it("falls back to the Plane handle when the identity is unknown", () => {
    const merged = mergeRecent([
      section(REMOTE, { kind: "ready", data: [row(REMOTE, "r", "5")] }),
      section("local", { kind: "ready", data: [row("local", "l", "5")] }),
    ]);
    // "0000000a-…" sorts before "local".
    expect(merged.map((item) => item.id)).toEqual(["r", "l"]);
  });

  it("compares 20-digit timestamps exactly", () => {
    expect(compareDecimal("100000000000000000000", "99999999999999999999")).toBe(1);
    // Both round to the same double; BigInt keeps them apart.
    expect(compareDecimal("9007199254740993", "9007199254740992")).toBe(1);
    expect(compareDecimal("7", "7")).toBe(0);
    expect(compareDecimal("oops", "1")).toBe(-1);
    const merged = mergeRecent([
      section("local", {
        kind: "ready",
        data: [row("local", "older", "99999999999999999999"), row("local", "newer", "100000000000000000000")],
      }),
    ]);
    expect(merged.map((item) => item.id)).toEqual(["newer", "older"]);
  });

  it("includes stale rows and skips Planes without data", () => {
    const merged = mergeRecent([
      section("local", { kind: "stale", data: [row("local", "kept", "1")], error: error("offline", "transport.offline") }),
      section(REMOTE, { kind: "offline", error: error("offline", "transport.offline") }),
    ]);
    expect(merged.map((item) => item.id)).toEqual(["kept"]);
  });
});

describe("retainNewest", () => {
  it("keeps the newest 4,096 rows across oldest-first pages", () => {
    let retained: ConversationRow[] = [];
    for (let page = 0; page < 20; page += 1) {
      const rows = Array.from({ length: 256 }, (_, index) => {
        const n = page * 256 + index;
        return row("local", `c${String(n).padStart(5, "0")}`, String(1_000 + n));
      });
      retained = retainNewest(retained, rows);
    }
    expect(retained).toHaveLength(4096);
    expect(retained[0].id).toBe("c05119");
    expect(retained.at(-1)?.id).toBe("c01024");
  });

  it("replaces a row seen again", () => {
    const retained = retainNewest([row("local", "a", "1")], [{ ...row("local", "a", "1"), title: "Renamed" }]);
    expect(retained).toEqual([{ ...row("local", "a", "1"), title: "Renamed" }]);
  });
});

describe("classifySection", () => {
  it.each([
    ["offline", "transport.offline", false, false, "offline"],
    ["offline", "transport.offline", false, true, "stale"],
    ["unavailable", "plane.jetd_unavailable", true, false, "offline"],
    ["unavailable", "plane.jetd_missing", false, false, "failed"],
    ["unavailable", "plane.jetd_missing", false, true, "stale"],
    ["unauthorized", "connection.unauthorized", false, true, "denied"],
    ["incompatible", "protocol.feature_unavailable", false, false, "unsupported"],
    ["incompatible", "protocol.remote_auth_required", false, true, "unsupported"],
    ["internal", "protocol.invalid_response", false, false, "failed"],
  ] as const)("%s %s (retryable %s, data %s) → %s", (category, code, retryable, hasData, kind) => {
    expect(classifySection(error(category, code, retryable), hasData)).toBe(kind);
  });

  it("chooses row copy by code", () => {
    expect(planeErrorCopy(error("unavailable", "identity.secret_store_locked", true), "host")).toBe(
      "Unlock your keyring to reach host.",
    );
    expect(planeErrorCopy(error("offline", "transport.offline"), "host")).toBe("host is offline.");
    expect(planeErrorCopy(error("incompatible", "protocol.feature_unavailable"), "host")).toBe(
      "Not available on host. Update Jet on host.",
    );
  });
});

describe("Recent status rows", () => {
  const label = (planeId: string) => (planeId === "local" ? "This computer" : "host");

  it("names every non-ready Plane with its own action", () => {
    const rows = recentStatusRows(
      [
        section("local", { kind: "ready", data: [] }),
        section("p-loading", { kind: "loading" }),
        section("p-offline", { kind: "offline", error: error("offline", "transport.offline") }),
        section("p-stale", { kind: "stale", data: [], error: error("offline", "transport.offline") }, { savedAtUnixMs: 0 }),
        section("p-denied", { kind: "denied", error: error("unauthorized", "connection.unauthorized") }),
        section("p-unsupported", { kind: "unsupported", error: error("incompatible", "protocol.incompatible") }),
        section("p-failed", { kind: "failed", error: error("internal", "protocol.invalid_response") }),
        section("p-partial", { kind: "ready", data: [] }, { chain: "partial", partial: "page_limit" }),
        section("p-locked", { kind: "offline", error: error("unavailable", "identity.secret_store_locked", true) }),
      ],
      label,
    );
    expect(rows.map(({ planeId, kind, action }) => [planeId, kind, action])).toEqual([
      ["p-loading", "loading", null],
      ["p-offline", "offline", "retry"],
      ["p-stale", "stale", "retry"],
      ["p-denied", "denied", "open_planes"],
      ["p-unsupported", "unsupported", "open_planes"],
      ["p-failed", "failed", "retry"],
      ["p-partial", "partial", "open_planes"],
      ["p-locked", "offline", "retry"],
    ]);
    const text = Object.fromEntries(rows.map((item) => [item.planeId, item.text]));
    expect(text["p-loading"]).toBe("Loading tasks from host…");
    expect(text["p-offline"]).toBe("host is offline.");
    expect(text["p-stale"]).toMatch(/^host is offline\. Showing tasks saved .+\.$/);
    expect(text["p-denied"]).toBe("host doesn't allow this computer.");
    expect(text["p-unsupported"]).toBe("Tasks from host need a newer Jet. Update Jet on host.");
    expect(text["p-failed"]).toBe("Tasks from host couldn't load.");
    expect(text["p-partial"]).toBe("Newest tasks from host may be missing.");
    expect(text["p-locked"]).toBe("Unlock your keyring to reach host.");
  });
});

describe("Search groups", () => {
  const result = (planeId: string, ids: string[], indexedThrough = "9"): ConversationSearchResult => ({
    planeId,
    cursor: "9",
    indexedThrough,
    hits: ids.map((id, index) => ({ conversationId: id, sequence: String(index), field: "name", excerpt: id })),
  });
  const planes = [
    { planeId: "local", label: "This computer" },
    { planeId: REMOTE, label: "host" },
  ];

  it("keeps each Plane's hit order and never interleaves Planes", () => {
    const groups = searchGroups(
      {
        [REMOTE]: { kind: "ready", result: result(REMOTE, ["r2", "r1"]) },
        local: { kind: "ready", result: result("local", ["l3", "l1", "l2"]) },
      },
      planes,
    );
    expect(groups.map((group) => group.planeId)).toEqual(["local", REMOTE]);
    expect(
      groups.map((group) =>
        group.state.kind === "ready" ? group.state.result.hits.map((hit) => hit.conversationId) : [],
      ),
    ).toEqual([["l3", "l1", "l2"], ["r2", "r1"]]);
    expect(groups.every((group) => group.showHeading)).toBe(true);
  });

  it("shows per-Plane states, indexing, filters and hides headings with one Plane", () => {
    const groups = searchGroups(
      {
        local: { kind: "ready", result: result("local", [], "3") },
        [REMOTE]: { kind: "unsupported", error: error("incompatible", "protocol.feature_unavailable") },
      },
      planes,
    );
    expect(groups.map((group) => group.text)).toEqual([
      "No matches on This computer",
      "Search isn't available on host. Update Jet on host.",
    ]);
    expect(groups[0].indexing).toBe(true);
    expect(searchGroups({ local: { kind: "searching" }, [REMOTE]: { kind: "searching" } }, planes, REMOTE)
      .map((group) => group.text)).toEqual(["Searching host…"]);
    const single = searchGroups({ local: { kind: "ready", result: result("local", []) } }, planes.slice(0, 1));
    expect(single[0]).toMatchObject({ showHeading: false, text: "No matching tasks" });
  });
});

describe("Plane status and attention", () => {
  it("counts failed, needs pairing, session ended, degraded, read-only and waiting claims", () => {
    const planes = [
      plane(),
      plane({ planeId: "a", connection: { state: "failed", error: error("unauthorized", "connection.unauthorized") } }),
      plane({ planeId: "b", connection: { state: "failed", error: error("unavailable", "identity.session_ended") } }),
      plane({ planeId: "c", security: "degraded" }),
      plane({ planeId: "d", store: "read_only" }),
      plane({ planeId: "e", connection: { state: "reconnecting", error: error("offline", "transport.offline") } }),
      plane({ planeId: "f", connection: { state: "idle" } }),
    ];
    expect(planes.map((item) => planeStatus(item).text)).toEqual([
      "Connected",
      "Needs pairing",
      "Session ended",
      "Security needs attention",
      "Read-only",
      "Reconnecting",
      "Not connected",
    ]);
    expect(planeAttentionCount(planes)).toBe(4);
    expect(planeAttentionCount(planes, { local: { awaitingConfirmation: true } })).toBe(5);
  });

  it("aggregates the sidebar status", () => {
    expect(aggregateStatus([plane()])).toEqual({ title: "This computer", detail: "Connected", tone: "ok" });
    expect(
      aggregateStatus([plane(), plane({ planeId: "a", connection: { state: "connecting" } }), plane({ planeId: "b" })]),
    ).toMatchObject({ title: "Planes", detail: "2 of 3 Planes connected", tone: "progress" });
  });

  it("shows only an 8-hex identity prefix", () => {
    expect(identityPrefix("ABCDEF01-2345-4678-9abc-def012345678")).toBe("abcdef01");
    expect(identityPrefix(null)).toBeNull();
    expect(identityPrefix("<script>")).toBeNull();
  });
});

describe("featureLabel", () => {
  const feature = (support: FeatureSupport["support"]): FeatureSupport => ({
    feature: "search",
    requiredMinor: 12,
    support,
  });

  it("covers unknown, unsupported, supported and at_most-only knowledge", () => {
    expect(featureLabel(feature("unknown"), { exact: null, atLeast: 1, atMost: null })).toBe("Checked when used");
    expect(featureLabel(feature("supported"), { exact: null, atLeast: 37, atMost: null })).toBe("Available");
    expect(featureLabel(feature("unsupported"), { exact: 9, atLeast: 9, atMost: 9 })).toBe(
      "Needs Jet protocol 1.12 (this Plane: 1.9)",
    );
    expect(featureLabel(feature("unsupported"), { exact: null, atLeast: 0, atMost: 0 })).toBe(
      "Needs Jet protocol 1.12 (this Plane: 1.0 or older)",
    );
  });
});
