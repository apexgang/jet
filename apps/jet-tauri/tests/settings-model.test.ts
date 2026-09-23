import { describe, expect, it } from "vitest";

import {
  LANDED_SECTIONS,
  PANES,
  landedPanes,
  paneOf,
  recoveryFor,
  resolveTarget,
  sectionHeadingId,
  sectionStateFor,
  settingsTargetForError,
  type SectionState,
} from "../src/lib/features/settings/model";
import type { PublicError } from "../src/lib/jet/bridge";

function error(code: string, category = "invalid_input", retryable = false, planeId: string | null = null): PublicError {
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

const REMOTE = "0000000a-0000-4000-8000-000000000002";

describe("settings targets", () => {
  it("lists every section once under the pane the shell expects", () => {
    const sections = PANES.flatMap((pane) => pane.sections.map((section) => [section.id, pane.id]));
    expect(Object.fromEntries(sections)).toEqual({
      appearance: "general",
      notifications: "general",
      restoration: "general",
      harnesses: "agents",
      extensions: "agents",
      accounts: "agents",
      usage: "agents",
      utility: "agents",
      projects: "work",
      delivery: "work",
      reviews: "work",
      schedules: "work",
      retention: "work",
      local_service: "connections",
      planes: "connections",
      execution: "safety",
      permissions: "safety",
      storage: "safety",
      audit: "safety",
    });
    expect(sections).toHaveLength(19);
    expect(PANES.map((pane) => pane.title)).toEqual(["General", "Agents", "Work", "Connections", "Safety and system"]);
  });

  it("shows only panes with a landed section", () => {
    expect(landedPanes().map((pane) => pane.id)).toEqual(["general", "connections"]);
    for (const section of LANDED_SECTIONS) expect(landedPanes().some((pane) => pane.id === paneOf(section))).toBe(true);
  });

  it("resolves targets to what exists in this build", () => {
    expect(resolveTarget({ pane: "connections", section: "planes" })).toEqual({ pane: "connections", section: "planes" });
    expect(resolveTarget({ pane: "general" })).toEqual({ pane: "general", section: null });
    expect(resolveTarget({ pane: "agents", section: "accounts" })).toEqual({ pane: "general", section: null });
    expect(resolveTarget({ pane: "general", section: "planes" })).toEqual({ pane: "general", section: null });
    expect(sectionHeadingId("local_service")).toBe("section-local-service");
  });

  it("maps landed error codes to their section", () => {
    expect(settingsTargetForError(error("notifications.permission_denied"))).toEqual({ pane: "general", section: "notifications" });
    for (const code of ["transport.offline", "protocol.incompatible", "protocol.feature_unavailable", "protocol.unsupported_minor"]) {
      expect(settingsTargetForError(error(code, "offline", true, "local"))).toEqual({ pane: "connections", section: "local_service" });
    }
    // A remote Plane's connection problem is shown in the Planes list, not the local service.
    expect(settingsTargetForError(error("transport.offline", "offline", true, REMOTE))).toEqual({ pane: "connections", section: "planes" });
  });

  it("returns null for unknown codes and for sections that have not landed", () => {
    expect(settingsTargetForError(error("conversation.not_found"))).toBeNull();
    for (const code of [
      "git.invalid_policy",
      "retention.grace_unreadable",
      "energy.budget_exhausted",
      "storage.disk_pressure",
      "craft.developer_mode_required",
      "security.audit_degraded",
      "account.not_found",
      "craft.disabled",
      "utility.consent_required",
      "review.consent_required",
      "extension.refused",
      "recovery.read_only",
    ]) {
      expect(settingsTargetForError(error(code)), code).toBeNull();
    }
  });
});

describe("section states", () => {
  it("classifies failures and names their recovery", () => {
    const cases: Array<[PublicError, SectionState<unknown>["kind"], string | null]> = [
      [error("transport.offline", "offline", true), "offline", "try_again"],
      [error("connection.unauthorized", "unauthorized"), "denied", null],
      [error("protocol.incompatible", "incompatible"), "unsupported", null],
      [error("protocol.feature_unavailable", "incompatible"), "unsupported", null],
      [error("protocol.unsupported_minor", "incompatible"), "unsupported", null],
      [error("security.audit_degraded", "conflict"), "failed", "open_audit"],
      [error("account.provider_unavailable", "unavailable"), "failed", "check_again"],
      [error("client.request_failed", "internal"), "failed", "try_again"],
      [error("setting.value_unsupported", "invalid_input"), "failed", null],
    ];
    for (const [failure, kind, recovery] of cases) {
      const state = sectionStateFor(failure, { value: 1 });
      expect(state.kind, failure.code).toBe(kind);
      expect(recoveryFor(state), failure.code).toBe(recovery);
    }
  });

  it("keeps the last trustworthy data while offline", () => {
    const state = sectionStateFor(error("transport.offline", "offline", true), { value: 1 });
    expect(state).toMatchObject({ kind: "offline", last: { value: 1 } });
    expect(recoveryFor({ kind: "ready", data: 1, freshness: "live", issues: [] })).toBeNull();
    expect(recoveryFor({ kind: "ready", data: 1, freshness: "changed", issues: [] })).toBe("check_again");
  });
});
