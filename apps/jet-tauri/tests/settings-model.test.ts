import { describe, expect, it } from "vitest";

import {
  LANDED_SECTIONS,
  MAX_SETTING_TEXT_BYTES,
  PANES,
  SETTING_KEYS,
  SETTING_PLACEMENT,
  CONSENT_FOR,
  consentState,
  emptyBindingLabel,
  isBindingKey,
  isConsentKey,
  isSensitive,
  landedTarget,
  planeRows,
  projectRows,
  sameSetting,
  sourceLabel,
  storedAt,
  textFits,
  utf8Bytes,
  valueText,
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
    expect(landedPanes().map((pane) => pane.id)).toEqual(["general", "agents", "work", "connections", "safety"]);
    for (const section of LANDED_SECTIONS) expect(landedPanes().some((pane) => pane.id === paneOf(section))).toBe(true);
  });

  it("resolves targets to what exists in this build", () => {
    expect(resolveTarget({ pane: "connections", section: "planes" })).toEqual({ pane: "connections", section: "planes" });
    expect(resolveTarget({ pane: "general" })).toEqual({ pane: "general", section: null });
    expect(resolveTarget({ pane: "agents", section: "accounts" })).toEqual({ pane: "agents", section: "accounts" });
    expect(resolveTarget({ pane: "work", section: "reviews" })).toEqual({ pane: "work", section: "reviews" });
    expect(resolveTarget({ pane: "agents", section: "extensions" })).toEqual({ pane: "agents", section: "extensions" });
    // A section of another pane opens the pane at its top.
    expect(resolveTarget({ pane: "work", section: "extensions" })).toEqual({ pane: "work", section: null });
    expect(resolveTarget({ pane: "safety", section: "audit", plane_id: REMOTE })).toEqual({ pane: "safety", section: "audit" });
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

  it("maps slice 2 codes to Work and Safety, carrying the failing Plane", () => {
    const cases: Array<[string, string, string]> = [
      ["git.invalid_policy", "work", "delivery"],
      ["git.policy_changed", "work", "delivery"],
      ["retention.grace_unreadable", "work", "retention"],
      ["energy.budget_exhausted", "safety", "execution"],
      ["energy.policy_unreadable", "safety", "execution"],
      ["storage.disk_pressure", "safety", "storage"],
      ["craft.developer_mode_required", "safety", "permissions"],
      ["security.audit_degraded", "safety", "audit"],
      ["review.audit_degraded", "safety", "audit"],
    ];
    for (const [code, pane, section] of cases) {
      expect(settingsTargetForError(error(code)), code).toEqual({ pane, section });
      expect(settingsTargetForError(error(code, "conflict", false, REMOTE)), code).toEqual({ pane, section, plane_id: REMOTE });
    }
  });

  it("maps slice 3 codes to Agents and Reviews, carrying the failing Plane", () => {
    const cases: Array<[string, string, string]> = [
      ["account.not_found", "agents", "accounts"],
      ["account.provider_unsupported", "agents", "accounts"],
      ["account.provider_unavailable", "agents", "accounts"],
      ["auto_continue.invalid_policy", "agents", "accounts"],
      ["review.credential_unavailable", "agents", "accounts"],
      ["utility.credential_unavailable", "agents", "accounts"],
      ["craft.disabled", "agents", "harnesses"],
      ["craft.revoked", "agents", "harnesses"],
      ["craft.update_pending", "agents", "harnesses"],
      ["craft.installation_stale", "agents", "harnesses"],
      ["craft.installation_failed", "agents", "harnesses"],
      ["utility.consent_required", "agents", "utility"],
      ["utility.disabled", "agents", "utility"],
      ["utility.binding_unavailable", "agents", "utility"],
      ["review.consent_required", "work", "reviews"],
      ["review.binding_unavailable", "work", "reviews"],
    ];
    for (const [code, pane, section] of cases) {
      expect(settingsTargetForError(error(code)), code).toEqual({ pane, section });
      expect(settingsTargetForError(error(code, "conflict", false, REMOTE)), code).toEqual({ pane, section, plane_id: REMOTE });
    }
    expect(landedTarget("accounts", "local")).toEqual({ pane: "agents", section: "accounts", plane_id: "local" });
    expect(landedTarget("reviews")).toEqual({ pane: "work", section: "reviews" });
  });

  it("maps slice 4 extension codes to Agents › Extensions by prefix", () => {
    for (const code of ["extension.refused", "extension.change_pending", "extension.preview_stale", "extension.unavailable"]) {
      expect(settingsTargetForError(error(code)), code).toEqual({ pane: "agents", section: "extensions" });
      expect(settingsTargetForError(error(code, "conflict", false, REMOTE)), code).toEqual({
        pane: "agents",
        section: "extensions",
        plane_id: REMOTE,
      });
    }
    // An exact code wins over the prefix.
    expect(settingsTargetForError(error("craft.disabled"))).toEqual({ pane: "agents", section: "harnesses" });
    expect(landedTarget("extensions", "local")).toEqual({ pane: "agents", section: "extensions", plane_id: "local" });
  });

  it("returns null for unknown codes and for sections that have not landed", () => {
    expect(settingsTargetForError(error("conversation.not_found"))).toBeNull();
    for (const code of ["recovery.read_only", "extensions.catalog_unreadable"]) {
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

describe("setting placement", () => {
  /** wave 3.2 §3.2, mirrored from jet-core `CATALOG`. */
  const DOCUMENTED: Record<string, string[]> = {
    "storage.disposable_mib": ["plane"],
    "git.message_instructions": ["plane"],
    "retention.trash_grace_days": ["plane"],
    "security.audit_retention_days": ["plane"],
    "utility.git_text": ["plane"],
    "utility.content_consent": ["plane"],
    "utility.account_binding": ["plane"],
    "utility.autodelete_compilation": ["plane"],
    "craft.developer_mode": ["plane"],
    "review.automatic": ["plane"],
    "review.account_binding": ["plane"],
    "review.cross_provider_consent": ["plane"],
    "artifact.max_mib": ["plane"],
    "artifact.run_mib": ["plane"],
    "energy.concurrency": ["plane"],
    "energy.low_power_concurrency": ["plane"],
    "energy.constrained": ["plane"],
    "energy.foreground_override": ["plane"],
    "git.auto_commit": ["project", "conversation"],
    "git.auto_branch": ["project", "conversation"],
    "git.auto_push": ["project", "conversation"],
    "git.auto_draft_pull_request": ["project", "conversation"],
    "git.branch_prefix": ["project", "conversation"],
    "utility.automatic_naming": ["plane", "project", "conversation"],
  };

  it("places all 24 keys with the documented scopes", () => {
    expect(SETTING_KEYS).toHaveLength(24);
    expect(Object.fromEntries(SETTING_KEYS.map((key) => [key, [...SETTING_PLACEMENT[key].scopes]]))).toEqual(DOCUMENTED);
    for (const key of SETTING_KEYS) {
      const placement = SETTING_PLACEMENT[key];
      expect(paneOf(placement.section), key).toBe(placement.pane);
    }
  });

  it("never lists Project-only Git keys on a Plane pane", () => {
    const planeKeys = PANES.flatMap((pane) => pane.sections.flatMap((section) => planeRows(section.id)));
    for (const key of ["git.auto_commit", "git.auto_branch", "git.auto_push", "git.auto_draft_pull_request", "git.branch_prefix"]) {
      expect(planeKeys).not.toContain(key);
    }
    expect(projectRows()).toEqual([
      "git.auto_commit",
      "git.auto_branch",
      "git.auto_push",
      "git.auto_draft_pull_request",
      "git.branch_prefix",
      "utility.automatic_naming",
    ]);
    expect(planeRows("execution")).toEqual([
      "energy.concurrency",
      "energy.low_power_concurrency",
      "energy.constrained",
      "energy.foreground_override",
    ]);
    expect(planeRows("storage")).toEqual(["storage.disposable_mib", "artifact.max_mib", "artifact.run_mib"]);
    expect(planeRows("delivery")).toEqual(["git.message_instructions", "utility.git_text"]);
    expect(planeRows("retention")).toEqual(["retention.trash_grace_days"]);
    expect(planeRows("audit")).toEqual(["security.audit_retention_days"]);
    expect(planeRows("permissions")).toEqual(["craft.developer_mode"]);
  });

  it("confirms exactly the sensitive keys", () => {
    expect(SETTING_KEYS.filter(isSensitive).sort()).toEqual(
      [
        "craft.developer_mode",
        "review.automatic",
        "review.account_binding",
        "review.cross_provider_consent",
        "utility.account_binding",
        "utility.content_consent",
        "energy.foreground_override",
        "git.auto_push",
        "git.auto_draft_pull_request",
        "security.audit_retention_days",
        "retention.trash_grace_days",
      ].sort(),
    );
    for (const key of SETTING_KEYS.filter(isSensitive)) expect(SETTING_PLACEMENT[key].consequence, key).toBeTruthy();
  });

  it("labels every source", () => {
    const projects = [{ id: "p1", name: "jet" }];
    expect(sourceLabel({ source: "built_in" }, projects)).toBe("Default");
    expect(sourceLabel({ source: "plane" }, projects)).toBe("Set for this Plane");
    expect(sourceLabel({ source: "project", projectId: "p1" }, projects)).toBe("Set for jet");
    expect(sourceLabel({ source: "project", projectId: "p2" }, projects)).toBe("Set for this Project");
    expect(sourceLabel({ source: "conversation", conversationId: "c" }, projects)).toBe("Set for one task");
    expect(storedAt({ source: "plane" }, { type: "plane" })).toBe(true);
    expect(storedAt({ source: "built_in" }, { type: "plane" })).toBe(false);
    expect(storedAt({ source: "plane" }, { type: "project", projectId: "p1" })).toBe(false);
    expect(storedAt({ source: "project", projectId: "p1" }, { type: "project", projectId: "p1" })).toBe(true);
  });

  it("counts text in UTF-8 bytes at the 2,048 limit", () => {
    expect(MAX_SETTING_TEXT_BYTES).toBe(2048);
    expect(utf8Bytes("é")).toBe(2);
    expect(utf8Bytes("🚀")).toBe(4);
    expect(textFits("git.branch_prefix", "é".repeat(1024))).toBe(true);
    expect(textFits("git.branch_prefix", `${"é".repeat(1024)}a`)).toBe(false);
    expect(textFits("git.branch_prefix", "🚀".repeat(512))).toBe(true);
    expect(textFits("git.branch_prefix", "a\nb")).toBe(false);
    expect(textFits("git.message_instructions", "a\n\tb")).toBe(true);
    expect(textFits("git.message_instructions", "a\u001bb")).toBe(false);
    expect(textFits("git.branch_prefix", "")).toBe(true);
  });

  it("describes consent by the exact binding it names", () => {
    const binding = "00000000-0000-4000-8000-0000000000b1";
    const other = "00000000-0000-4000-8000-0000000000b2";
    expect(consentState("", "")).toBe("none");
    expect(consentState("", binding)).toBe("none");
    expect(consentState(binding, binding)).toBe("granted");
    expect(consentState(binding, "")).toBe("missing");
    expect(consentState(binding, other)).toBe("mismatch");
    expect(CONSENT_FOR["review.account_binding"]).toBe("review.cross_provider_consent");
    expect(CONSENT_FOR["utility.account_binding"]).toBe("utility.content_consent");
    for (const key of SETTING_KEYS) {
      expect(isBindingKey(key) || isConsentKey(key), key).toBe(key in CONSENT_FOR || Object.values(CONSENT_FOR).includes(key as never));
    }
  });

  it("shows binding and consent values by account label, never by UUID", () => {
    const binding = "00000000-0000-4000-8000-0000000000b1";
    const bindings = [{ id: binding, label: "Codex login" }];
    expect(valueText("review.account_binding", { type: "text", value: "" }, bindings)).toBe("Each run's own account");
    expect(valueText("utility.account_binding", { type: "text", value: "" }, bindings)).toBe("No account");
    expect(valueText("review.account_binding", { type: "text", value: binding }, bindings)).toBe("Codex login");
    expect(valueText("review.account_binding", { type: "text", value: binding }, [])).toBe(
      "An account that's no longer connected",
    );
    expect(valueText("review.cross_provider_consent", { type: "text", value: binding }, bindings)).toBe(
      "Allowed for Codex login",
    );
    expect(valueText("utility.content_consent", { type: "text", value: "" }, bindings)).toBe("Not allowed");
    expect(emptyBindingLabel("review.account_binding")).toBe("Each run's own account");
    expect(planeRows("reviews")).toEqual(["review.automatic", "review.account_binding", "review.cross_provider_consent"]);
    expect([...planeRows("utility")].sort()).toEqual([
      "utility.account_binding",
      "utility.autodelete_compilation",
      "utility.automatic_naming",
      "utility.content_consent",
    ]);
  });

  it("describes values in plain words", () => {
    expect(valueText("energy.constrained", { type: "flag", value: true })).toBe("On");
    expect(valueText("retention.trash_grace_days", { type: "count", value: 30 })).toBe("30 days");
    expect(valueText("git.branch_prefix", { type: "text", value: "" })).toBe("Empty");
    expect(valueText("git.branch_prefix", { type: "undisplayable" })).toBe("A value that can't be displayed");
    expect(valueText("git.branch_prefix", null)).toBe("The inherited value");
    const plane = { key: "energy.constrained" as const, value: { type: "flag" as const, value: true }, source: { source: "plane" as const } };
    expect(sameSetting(plane, { ...plane })).toBe(true);
    expect(sameSetting(plane, { ...plane, source: { source: "built_in" } })).toBe(false);
  });
});
