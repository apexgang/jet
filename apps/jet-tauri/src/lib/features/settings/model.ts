import type { PublicError } from "$lib/jet/bridge";
import { LOCAL_PLANE } from "$lib/jet/planes";
import type { SettingsPane, SettingsSection, SettingsTarget } from "$lib/jet/settings-window";

/** Pane and section titles, in design-language order (docs/design-language.md 216-222). */
export const PANES: ReadonlyArray<{
  id: SettingsPane;
  title: string;
  sections: ReadonlyArray<{ id: SettingsSection; title: string }>;
}> = [
  {
    id: "general",
    title: "General",
    sections: [
      { id: "appearance", title: "Appearance" },
      { id: "notifications", title: "Notifications" },
      { id: "restoration", title: "Restoration" },
    ],
  },
  {
    id: "agents",
    title: "Agents",
    sections: [
      { id: "harnesses", title: "Harnesses" },
      { id: "extensions", title: "Extensions" },
      { id: "accounts", title: "Accounts" },
      { id: "usage", title: "Usage" },
      { id: "utility", title: "Utility" },
    ],
  },
  {
    id: "work",
    title: "Work",
    sections: [
      { id: "projects", title: "Projects" },
      { id: "delivery", title: "Delivery" },
      { id: "reviews", title: "Reviews" },
      { id: "schedules", title: "Schedules" },
      { id: "retention", title: "Retention" },
    ],
  },
  {
    id: "connections",
    title: "Connections",
    sections: [
      { id: "local_service", title: "Local service" },
      { id: "planes", title: "Planes" },
    ],
  },
  {
    id: "safety",
    title: "Safety and system",
    sections: [
      { id: "execution", title: "Execution" },
      { id: "permissions", title: "Permissions" },
      { id: "storage", title: "Storage" },
      { id: "audit", title: "Audit" },
    ],
  },
];

/**
 * Sections that exist in this build. Each Wave 3.2 slice extends it, so no
 * deep link and no pane opens onto a section that has not landed.
 */
export const LANDED_SECTIONS: ReadonlySet<SettingsSection> = new Set<SettingsSection>([
  "appearance",
  "notifications",
  "restoration",
  "local_service",
  "planes",
]);

export function paneOf(section: SettingsSection): SettingsPane {
  const pane = PANES.find((candidate) => candidate.sections.some((entry) => entry.id === section));
  if (!pane) throw new Error(`Unknown Settings section ${section}`);
  return pane.id;
}

export function paneTitle(pane: SettingsPane): string {
  return PANES.find((candidate) => candidate.id === pane)?.title ?? "Settings";
}

/** Panes shown in the navigation: those with at least one landed section. */
export function landedPanes(): typeof PANES {
  return PANES.filter((pane) => pane.sections.some((section) => LANDED_SECTIONS.has(section.id)));
}

/** A section's heading id; deep links focus it. */
export function sectionHeadingId(section: SettingsSection): string {
  return `section-${section.replace(/_/g, "-")}`;
}

/**
 * The pane and section the window can actually show for a target. A pane or
 * section that has not landed falls back to General or to the pane's top.
 */
export function resolveTarget(target: SettingsTarget): {
  pane: SettingsPane;
  section: SettingsSection | null;
} {
  const section = target.section ?? null;
  if (section && LANDED_SECTIONS.has(section) && paneOf(section) === target.pane) {
    return { pane: target.pane, section };
  }
  if (landedPanes().some((pane) => pane.id === target.pane)) {
    return { pane: target.pane, section: null };
  }
  return { pane: "general", section: null };
}

const EXACT_TARGETS: ReadonlyArray<[readonly string[], SettingsPane, SettingsSection]> = [
  [["notifications.permission_denied"], "general", "notifications"],
  [
    ["transport.offline", "protocol.incompatible", "protocol.feature_unavailable", "protocol.unsupported_minor"],
    "connections",
    "local_service",
  ],
  [["git.invalid_policy", "git.policy_changed"], "work", "delivery"],
  [["retention.grace_unreadable"], "work", "retention"],
  [["energy.budget_exhausted", "energy.policy_unreadable"], "safety", "execution"],
  [["storage.disk_pressure"], "safety", "storage"],
  [["craft.developer_mode_required"], "safety", "permissions"],
  [["security.audit_degraded", "review.audit_degraded"], "safety", "audit"],
  [
    [
      "account.not_found",
      "account.provider_unsupported",
      "account.provider_unavailable",
      "auto_continue.invalid_policy",
      "review.credential_unavailable",
      "utility.credential_unavailable",
    ],
    "agents",
    "accounts",
  ],
  [
    ["craft.disabled", "craft.revoked", "craft.update_pending", "craft.installation_stale", "craft.installation_failed"],
    "agents",
    "harnesses",
  ],
  [["utility.consent_required", "utility.disabled", "utility.binding_unavailable"], "agents", "utility"],
  [["review.consent_required", "review.binding_unavailable"], "work", "reviews"],
];

const PREFIX_TARGETS: ReadonlyArray<[string, SettingsPane, SettingsSection]> = [
  ["extension.", "agents", "extensions"],
];

/** Panes whose settings belong to one Plane; a link carries that Plane. */
const PLANE_PANES: ReadonlySet<SettingsPane> = new Set<SettingsPane>(["agents", "work", "safety"]);

/**
 * The Settings location that can fix an error, or null. Exact code first,
 * then prefix, then filtered by `LANDED_SECTIONS` (wave 3.2 §7.6).
 */
export function settingsTargetForError(error: Pick<PublicError, "code" | "planeId">): SettingsTarget | null {
  const exact = EXACT_TARGETS.find(([codes]) => codes.includes(error.code));
  const prefix = exact ? null : PREFIX_TARGETS.find(([start]) => error.code.startsWith(start));
  const match = exact ?? prefix;
  if (!match) return null;
  let [, pane, section] = match;
  // The local service summary describes this computer only; a remote Plane's
  // connection problem is shown in the Planes list.
  if (section === "local_service" && error.planeId && error.planeId !== LOCAL_PLANE) {
    section = "planes";
  }
  if (!LANDED_SECTIONS.has(section)) return null;
  return PLANE_PANES.has(pane) && error.planeId
    ? { pane, section, plane_id: error.planeId }
    : { pane, section };
}

export type SectionIssue = { section: string; error: PublicError };

/**
 * What one Settings section can show (design-language 164). `last` keeps the
 * last trustworthy data visible while the Plane is unreachable.
 */
export type SectionState<T> =
  | { kind: "loading"; last: T | null }
  | { kind: "empty"; cursor: string }
  | { kind: "ready"; data: T; freshness: "live" | "changed" | "stale"; issues: SectionIssue[] }
  | { kind: "offline"; last: T | null; error: PublicError }
  | { kind: "denied"; last: T | null; error: PublicError }
  | { kind: "unsupported"; error: PublicError }
  | { kind: "failed"; last: T | null; error: PublicError };

export type RecoveryAction = "try_again" | "check_again" | "open_audit" | null;

const UNSUPPORTED_CODES = new Set(["protocol.feature_unavailable", "protocol.unsupported_minor", "protocol.incompatible"]);

function isOffline(error: PublicError): boolean {
  return error.category === "offline" || (error.category === "unavailable" && error.retryable);
}

/** The state a failed section read leaves the section in. */
export function sectionStateFor<T>(error: PublicError, last: T | null): SectionState<T> {
  if (error.category === "incompatible" || UNSUPPORTED_CODES.has(error.code)) {
    return { kind: "unsupported", error };
  }
  if (error.category === "unauthorized") return { kind: "denied", last, error };
  if (isOffline(error)) return { kind: "offline", last, error };
  return { kind: "failed", last, error };
}

function credentialProblem(code: string): boolean {
  return code.startsWith("account.") || code.endsWith(".credential_unavailable");
}

/** The named recovery a section state offers, if any. */
export function recoveryFor(state: SectionState<unknown>): RecoveryAction {
  switch (state.kind) {
    case "offline":
      return "try_again";
    case "failed":
      if (state.error.code === "security.audit_degraded") return "open_audit";
      if (credentialProblem(state.error.code)) return "check_again";
      return state.error.retryable || state.error.category === "internal" ? "try_again" : null;
    case "ready":
      return state.freshness === "live" ? null : "check_again";
    default:
      return null;
  }
}

/** The data a state can still show, fresh or not. */
export function sectionData<T>(state: SectionState<T>): T | null {
  switch (state.kind) {
    case "ready":
      return state.data;
    case "loading":
    case "offline":
    case "denied":
    case "failed":
      return state.last;
    default:
      return null;
  }
}
