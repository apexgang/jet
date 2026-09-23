import { describe, expect, it } from "vitest";

import type { PublicError } from "../src/lib/jet/bridge";
import type { RetentionProtection, TrashEntry, TrashReason, TrashView } from "../src/lib/jet/retention";
import {
  CAPPED_TEXT,
  NAMES_PER_CALL,
  auditLine,
  emptyText,
  formatDate,
  modeConsequence,
  moveRefusalText,
  namesToResolve,
  protectionLine,
  reasonLabel,
  restoreActionText,
  restoreRefusalReloads,
  restoreRefusalText,
  retentionPolicyLine,
  sectionFromError,
  sectionFromView,
  sectionText,
  sectionView,
  trashStatus,
} from "../src/lib/features/trash/model";

const DAY = 86_400_000;
const NOW = Date.UTC(2026, 8, 1, 12, 0, 0);

function entry(id: string, overrides: Partial<TrashEntry> = {}): TrashEntry {
  return {
    conversationId: id,
    reason: "manual_forget",
    trashedAtUnixMs: String(NOW - DAY),
    expiresAtUnixMs: String(NOW + 29 * DAY),
    restorable: true,
    ...overrides,
  };
}

function error(code: string, category = "conflict", protocolLimit: PublicError["protocolLimit"] = null): PublicError {
  return {
    category,
    code,
    message: "Refused.",
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit,
    planeId: null,
  };
}

const view = (entries: TrashEntry[], graceDays: number | null = 30): TrashView => ({
  planeId: "local",
  planeLabel: "This computer",
  cursor: "4",
  graceDays,
  capped: entries.length === 256,
  entries,
});

describe("Jet Trash model", () => {
  it("labels every reason and every protection", () => {
    const reasons: Record<TrashReason, string> = {
      manual_forget: "You forgot it",
      automatic_forget: "Forgotten after its last activity ended",
      delete_everywhere: "You deleted it everywhere",
      autodelete_rule: "An auto-delete rule matched it",
      autodelete_everywhere: "An auto-delete rule matched it (delete everywhere)",
      plane_transfer: "Moved to another Plane",
    };
    for (const [reason, label] of Object.entries(reasons)) {
      expect(reasonLabel(reason as TrashReason)).toBe(label);
    }
    const protections: RetentionProtection[] = [
      "active_run",
      "pending_turn",
      "enabled_schedule",
      "dirty_workspace",
      "unpushed_work",
      "unresolved_effect",
    ];
    const lines = protections.map(protectionLine);
    expect(new Set(lines).size).toBe(protections.length);
    expect(protectionLine("active_run")).toBe("This task has activity in progress.");
    expect(protectionLine("unpushed_work")).toContain("deleted with the task");
  });

  it("formats concrete dates next to relative phrasing with a fixed now", () => {
    const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
    const absolute = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
    const inMonth = formatDate(String(NOW + 29 * DAY), NOW);
    expect(inMonth.relative).toBe(relative.format(29, "day"));
    expect(inMonth.absolute).toBe(absolute.format(new Date(NOW + 29 * DAY)));
    expect(inMonth.iso).toBe(new Date(NOW + 29 * DAY).toISOString());
    expect(formatDate(NOW + 3 * 3_600_000, NOW).relative).toBe(relative.format(3, "hour"));
    expect(formatDate(NOW - 2 * DAY, NOW).relative).toBe(relative.format(-2, "day"));
  });

  it("tells scheduled, due and tombstone rows apart", () => {
    expect(trashStatus(entry("a"), NOW)).toBe("scheduled");
    expect(trashStatus(entry("a", { expiresAtUnixMs: String(NOW) }), NOW)).toBe("due");
    expect(trashStatus(entry("a", { reason: "plane_transfer", restorable: false }), NOW)).toBe("tombstone");
  });

  it("discloses the retention policy only for tasks Jet forgets itself", () => {
    expect(retentionPolicyLine("retain")).toBeNull();
    expect(retentionPolicyLine("forget_after_final_run")).toBe("Jet forgets this task after its last activity ends.");
  });

  it("asks for names the loaded list lacks, at most 32 per call", () => {
    const entries = Array.from({ length: 70 }, (_, index) => entry(`c${index}`));
    entries.push(entry("c0"));
    const known = new Set(["c1", "c2"]);
    const chunks = namesToResolve(entries, (id) => known.has(id));
    expect(chunks.map((chunk) => chunk.length)).toEqual([NAMES_PER_CALL, NAMES_PER_CALL, 4]);
    const asked = chunks.flat();
    expect(asked).not.toContain("c1");
    expect(new Set(asked).size).toBe(asked.length);
    expect(namesToResolve([entry("c1")], (id) => known.has(id))).toEqual([]);
  });

  it("classifies sections and keeps the last list while offline", () => {
    const loaded = view([entry("a")]);
    expect(sectionFromView(loaded, NOW)).toEqual({ kind: "ready", view: loaded, freshness: "live", loadedAt: NOW });
    expect(sectionFromView(view([]), NOW).kind).toBe("empty");
    expect(sectionFromError(error("unauthorized", "unauthorized"), loaded).kind).toBe("denied");
    expect(
      sectionFromError(error("protocol.feature_unavailable", "incompatible", { requiredMinor: 39, negotiatedMinor: 30 }), null).kind,
    ).toBe("unsupported");
    const offline = sectionFromError(error("transport.offline", "offline"), loaded);
    expect(offline).toEqual({ kind: "offline", last: loaded });
    expect(sectionView(offline)).toBe(loaded);
    expect(sectionText(offline, "Build box")).toContain("Restore is unavailable until it reconnects");
    expect(sectionText({ kind: "unsupported", error: error("x") }, "Build box")).toBe(
      "Build box doesn't support Jet Trash. Update Jet on that Plane.",
    );
  });

  it("states the grace period when the Plane gave it", () => {
    expect(emptyText(view([], 30))).toBe(
      "Jet Trash on This computer is empty. Tasks you forget or delete wait here for 30 days before Jet deletes them.",
    );
    expect(emptyText(view([], null))).toBe(
      "Jet Trash on This computer is empty. Tasks you forget or delete wait here before Jet deletes them.",
    );
    expect(CAPPED_TEXT).toBe("Showing the 256 tasks deleted soonest. More may be in Jet Trash.");
  });

  it("explains refusals in plain words and reloads when Trash changed", () => {
    expect(restoreRefusalText(error("retention.not_trashed"), "Build box")).toBe("This task is no longer in Jet Trash.");
    expect(restoreRefusalText(error("recovery.read_only", "unavailable"), "Build box")).toBe(
      "Build box is in read-only recovery.",
    );
    expect(restoreRefusalReloads(error("retention.trash_unknown"))).toBe(true);
    expect(restoreRefusalReloads(error("security.audit_degraded"))).toBe(false);
    expect(restoreActionText({ kind: "idle" }, "Build box")).toBeNull();
    expect(restoreActionText({ kind: "uncertain", error: error("transport.offline") }, "Build box")).toContain(
      "won't be applied twice",
    );
    expect(moveRefusalText(error("retention.live_work"))).toContain("choose Delete everywhere");
    expect(moveRefusalText(error("retention.review_stale"))).toContain("Review again");
  });

  it("describes each mode's consequence with an estimated date", () => {
    expect(modeConsequence("forget", { graceDays: null }, NOW)).toBe(
      "Jet deletes its copy of this task after the Plane's grace period. The Harness keeps its own history. You can restore the task until then.",
    );
    const everywhere = modeConsequence("delete_everywhere", { graceDays: 30 }, NOW);
    expect(everywhere).toContain("on about");
    expect(everywhere).toContain("No installed Harness supports this yet; Jet records the request.");
    expect(everywhere).toContain("stopped work doesn't restart");
    expect(auditLine("0")).toBeNull();
    expect(auditLine("3")).toBe(
      "3 security audit records mention this task. They stay, without its contents, until they expire.",
    );
  });
});
