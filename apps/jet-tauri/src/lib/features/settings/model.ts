import type { PublicError } from "$lib/jet/bridge";
import { LOCAL_PLANE } from "$lib/jet/planes";
import type { ResolvedSetting, SettingKeyId, SettingSource, SettingValue, SettingsReview } from "$lib/jet/settings";
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
      { id: "recovery", title: "Recovery" },
      { id: "diagnostics", title: "Diagnostics" },
      { id: "audit", title: "Audit" },
      { id: "versions", title: "Versions and capabilities" },
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
  "harnesses",
  "extensions",
  "accounts",
  "usage",
  "utility",
  "local_service",
  "planes",
  "projects",
  "delivery",
  "reviews",
  "schedules",
  "retention",
  "execution",
  "permissions",
  "storage",
  "diagnostics",
  "audit",
  "versions",
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
  [["security.audit_degraded", "review.audit_degraded", "audit.export_required"], "safety", "audit"],
  [
    [
      "recovery.read_only",
      "recovery.deletion_ledger_corrupt",
      "recovery.restore_failed",
      "recovery.snapshot_gone",
      "recovery.purge_unavailable",
      "recovery.not_read_only_local",
    ],
    "safety",
    "recovery",
  ],
  [["security.gap_unknown"], "safety", "diagnostics"],
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

/**
 * A deep link to one section, or null while that section has not landed.
 * A Plane pane's link carries the Plane it is about.
 */
export function landedTarget(section: SettingsSection, planeId: string | null = null): SettingsTarget | null {
  if (!LANDED_SECTIONS.has(section)) return null;
  const pane = paneOf(section);
  return PLANE_PANES.has(pane) && planeId ? { pane, section, plane_id: planeId } : { pane, section };
}

/** Panes whose settings belong to one Plane; a link carries that Plane. */
export const PLANE_PANES: ReadonlySet<SettingsPane> = new Set<SettingsPane>(["agents", "work", "safety"]);

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

/** The same state with only the issues one sub-section renders. */
export function withIssues<T>(state: SectionState<T>, sections: readonly string[]): SectionState<T> {
  if (state.kind !== "ready") return state;
  return { ...state, issues: state.issues.filter((issue) => sections.includes(issue.section)) };
}

/** The data a state can still show, fresh or not. */
/** Marks ready data changed elsewhere or stale; a stale view stays stale until reloaded. */
export function withFreshness<T>(state: SectionState<T>, freshness: "changed" | "stale"): SectionState<T> {
  if (state.kind !== "ready") return state;
  if (freshness === "changed" && state.freshness === "stale") return state;
  return { ...state, freshness };
}

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

// ---------------------------------------------------------------------------
// Setting rows (wave 3.2 §7.2)
// ---------------------------------------------------------------------------

export type ScopeKind = "plane" | "project" | "conversation";

export type SettingPlacement = {
  /** Mirrors the shell's `SETTING_SCOPES` (jet-core `CATALOG`). */
  scopes: readonly ScopeKind[];
  pane: SettingsPane;
  section: SettingsSection;
  label: string;
  help: string;
  /** Sensitive rows confirm through a review dialog before applying. */
  sensitive: boolean;
  /** What confirming means, shown in the review dialog of sensitive rows. */
  consequence?: string;
  /** What confirming means when a flag or consent is turned off. */
  consequenceOff?: string;
  /** Lowering the value deletes data for good: the review starts on Cancel. */
  destructiveDecrease?: boolean;
  unit?: "MiB" | "days" | "tasks";
};

const PLANE: readonly ScopeKind[] = ["plane"];
const PROJECT: readonly ScopeKind[] = ["project", "conversation"];
const EVERY: readonly ScopeKind[] = ["plane", "project", "conversation"];

const PUSH_DISCLOSURE = "Jet will push to the task's branch without forcing.";

/**
 * Where each Setting is shown. Placement never comes from a snapshot: a
 * Plane resolves every key its minor names, at every scope.
 */
export const SETTING_PLACEMENT: Readonly<Record<SettingKeyId, SettingPlacement>> = {
  "storage.disposable_mib": {
    scopes: PLANE, pane: "safety", section: "storage", sensitive: false, unit: "MiB",
    label: "Space for disposable files",
    help: "Caches and build output Jet may delete to free space.",
  },
  "artifact.max_mib": {
    scopes: PLANE, pane: "safety", section: "storage", sensitive: false, unit: "MiB",
    label: "Largest single artifact",
    help: "Bigger outputs from a task aren't kept.",
  },
  "artifact.run_mib": {
    scopes: PLANE, pane: "safety", section: "storage", sensitive: false, unit: "MiB",
    label: "New artifacts per run",
    help: "The most new output one run may keep.",
  },
  "energy.concurrency": {
    scopes: PLANE, pane: "safety", section: "execution", sensitive: false, unit: "tasks",
    label: "Maximum tasks at once",
    help: "Tasks beyond this wait their turn.",
  },
  "energy.low_power_concurrency": {
    scopes: PLANE, pane: "safety", section: "execution", sensitive: false, unit: "tasks",
    label: "Tasks at once on battery or low power",
    help: "Used when this Plane runs on battery or saves power.",
  },
  "energy.constrained": {
    scopes: PLANE, pane: "safety", section: "execution", sensitive: false,
    label: "Always use the low-power limit",
    help: "Keeps the lower limit even on mains power.",
  },
  "energy.foreground_override": {
    scopes: PLANE, pane: "safety", section: "execution", sensitive: true,
    label: "Let my own tasks run above the limit",
    help: "Tasks you start yourself don't wait for the limit.",
    consequence: "Tasks you start can use more of this Plane's power and memory than the limit allows.",
    consequenceOff: "Tasks you start wait for the limit like any other task.",
  },
  "craft.developer_mode": {
    scopes: PLANE, pane: "safety", section: "permissions", sensitive: true,
    label: "Allow local and source-built Harness packages",
    help: "Needed to add a Craft from files on this Plane.",
    consequence: "Local packages aren't verified. They run with your user account's permissions.",
    consequenceOff: "Local and source-built packages can't be added on this Plane.",
  },
  "security.audit_retention_days": {
    scopes: PLANE, pane: "safety", section: "audit", sensitive: true, unit: "days",
    label: "Keep the security audit for",
    help: "This Plane sets the shortest period it allows.",
    consequence: "Audit records older than this are deleted and can't be reviewed later.",
    destructiveDecrease: true,
  },
  "retention.trash_grace_days": {
    scopes: PLANE, pane: "work", section: "retention", sensitive: true, unit: "days",
    label: "Keep tasks in Jet Trash for",
    help: "After this, tasks in Jet Trash are deleted for good.",
    consequence: "Tasks already in Jet Trash longer than this are deleted for good.",
    destructiveDecrease: true,
  },
  "git.message_instructions": {
    scopes: PLANE, pane: "work", section: "delivery", sensitive: false,
    label: "Instructions for commit messages and pull requests",
    help: "Jet follows these when it writes Git text on this Plane.",
  },
  "utility.git_text": {
    scopes: PLANE, pane: "work", section: "delivery", sensitive: false,
    label: "Write commit messages and pull request text automatically",
    help: "Uses the account chosen for naming and summaries.",
  },
  "git.auto_commit": {
    scopes: PROJECT, pane: "work", section: "projects", sensitive: false,
    label: "Commit changes after each turn",
    help: "Jet commits a task's changes when a turn succeeds.",
  },
  "git.auto_branch": {
    scopes: PROJECT, pane: "work", section: "projects", sensitive: false,
    label: "Create a branch for each task",
    help: "Created after the first successful turn.",
  },
  "git.auto_push": {
    scopes: PROJECT, pane: "work", section: "projects", sensitive: true,
    label: "Push each task's branch",
    help: PUSH_DISCLOSURE,
    consequence: `${PUSH_DISCLOSURE} Anyone with access to the remote sees the changes.`,
    consequenceOff: "Jet stops pushing task branches. Branches already pushed stay on the remote.",
  },
  "git.auto_draft_pull_request": {
    scopes: PROJECT, pane: "work", section: "projects", sensitive: true,
    label: "Open a draft pull request for each task",
    help: `${PUSH_DISCLOSURE} The draft stays up to date on GitHub.`,
    consequence: `${PUSH_DISCLOSURE} A draft pull request is created on GitHub and kept up to date.`,
    consequenceOff: "Jet stops opening draft pull requests. Pull requests already opened stay on GitHub.",
  },
  "git.branch_prefix": {
    scopes: PROJECT, pane: "work", section: "projects", sensitive: false,
    label: "Branch name prefix",
    help: "Starts the name of each branch Jet proposes.",
  },
  "utility.automatic_naming": {
    scopes: EVERY, pane: "agents", section: "utility", sensitive: false,
    label: "Name tasks automatically",
    help: "Jet suggests a name after the first turn.",
  },
  "utility.account_binding": {
    scopes: PLANE, pane: "agents", section: "utility", sensitive: true,
    label: "Account used for naming and summaries",
    help: "Jet uses this account for short background work.",
    consequence: "Changing the account doesn't carry over permission to send task content.",
  },
  "utility.content_consent": {
    scopes: PLANE, pane: "agents", section: "utility", sensitive: true,
    label: "Allow Jet to send task content for naming and summaries",
    help: "Applies to the chosen account only.",
    consequence: "Task content is sent to the chosen account's provider.",
    consequenceOff: "Jet stops sending task content to this account for naming and summaries.",
  },
  "utility.autodelete_compilation": {
    scopes: PLANE, pane: "agents", section: "utility", sensitive: false,
    label: "Help write auto-delete rules",
    help: "Turns a description into an auto-delete rule for you to review.",
  },
  "review.automatic": {
    scopes: PLANE, pane: "work", section: "reviews", sensitive: true,
    label: "Let Jet review eligible approval requests",
    help: "Requests Jet can't decide still wait for you.",
    consequence: "Jet decides eligible approval requests for you on this Plane.",
    consequenceOff: "Every approval request on this Plane waits for you.",
  },
  "review.account_binding": {
    scopes: PLANE, pane: "work", section: "reviews", sensitive: true,
    label: "Reviewer account",
    help: "Empty uses each run's own account.",
    consequence: "Changing the reviewer doesn't carry over permission to send task content.",
  },
  "review.cross_provider_consent": {
    scopes: PLANE, pane: "work", section: "reviews", sensitive: true,
    label: "Allow Jet to send task content to the reviewer account",
    help: "Applies to the chosen reviewer only.",
    consequence: "Task content is sent to the reviewer account's provider.",
    consequenceOff: "Jet stops sending task content to the reviewer account.",
  },
};

export const SETTING_KEYS = Object.keys(SETTING_PLACEMENT) as SettingKeyId[];

/** Keys a Plane-scope section renders, in table order. */
export function planeRows(section: SettingsSection): SettingKeyId[] {
  return SETTING_KEYS.filter((key) => {
    const placement = SETTING_PLACEMENT[key];
    return placement.section === section && placement.scopes.includes("plane");
  });
}

/** Keys the Projects section renders at Project scope. */
export function projectRows(): SettingKeyId[] {
  const order: SettingKeyId[] = [
    "git.auto_commit",
    "git.auto_branch",
    "git.auto_push",
    "git.auto_draft_pull_request",
    "git.branch_prefix",
    "utility.automatic_naming",
  ];
  return order.filter((key) => SETTING_PLACEMENT[key].scopes.includes("project"));
}

/** The value shape a key holds (jet-core `CATALOG` built-ins). */
export function valueKind(key: SettingKeyId): "flag" | "count" | "text" {
  if (SETTING_PLACEMENT[key].unit) return "count";
  switch (key) {
    case "git.branch_prefix":
    case "git.message_instructions":
    case "utility.account_binding":
    case "utility.content_consent":
    case "review.account_binding":
    case "review.cross_provider_consent":
      return "text";
    default:
      return "flag";
  }
}

export function isSensitive(key: SettingKeyId): boolean {
  return SETTING_PLACEMENT[key].sensitive;
}

/**
 * The consequence line of a review, for the direction it changes in.
 * Clearing a flag restores an inherited value whose direction isn't known
 * here, so it shows none rather than a wrong one.
 */
export function reviewConsequence(key: SettingKeyId, review: SettingsReview): string | undefined {
  const placement = SETTING_PLACEMENT[key];
  const after = review.after;
  if (isConsentKey(key)) {
    const granted = after?.type === "text" && after.value !== "";
    return granted ? placement.consequence : placement.consequenceOff;
  }
  if (valueKind(key) === "flag") {
    if (after?.type !== "flag") return undefined;
    return after.value ? placement.consequence : placement.consequenceOff;
  }
  return placement.consequence;
}

/**
 * Whether confirming may delete data for good: a lower retention, or a
 * value that can't be compared. Such reviews start on Cancel.
 */
export function reviewDestructive(key: SettingKeyId, review: SettingsReview): boolean {
  if (!SETTING_PLACEMENT[key].destructiveDecrease) return false;
  const before = review.before?.value;
  const after = review.after;
  if (before?.type !== "count" || after?.type !== "count") return true;
  return after.value < before.value;
}

/** Where a value comes from, in the vocabulary of the row's badge. */
export function sourceLabel(source: SettingSource, projects: ReadonlyArray<{ id: string; name: string }>): string {
  switch (source.source) {
    case "built_in":
      return "Default";
    case "plane":
      return "Set for this Plane";
    case "project": {
      const project = projects.find((candidate) => candidate.id === source.projectId);
      return `Set for ${project?.name ?? "this Project"}`;
    }
    case "conversation":
      return "Set for one task";
  }
}

/** Whether the row's own scope stores the value, so it can be cleared. */
export function storedAt(source: SettingSource, scope: SettingRowScope): boolean {
  return scope.type === "plane"
    ? source.source === "plane"
    : source.source === "project" && source.projectId === scope.projectId;
}

export type SettingRowScope = { type: "plane" } | { type: "project"; projectId: string };

/** "Use default" on the Plane, "Use Plane value" on a Project. */
export function clearLabel(key: SettingKeyId, scope: SettingRowScope): string {
  return scope.type === "project" && SETTING_PLACEMENT[key].scopes.includes("plane") ? "Use Plane value" : "Use default";
}

export const MAX_SETTING_TEXT_BYTES = 2_048;

/** UTF-8 length, the unit the Plane limits text by. */
export function utf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Text a row may send: the Plane's limit, and no control characters. */
export function textFits(key: SettingKeyId, text: string): boolean {
  if (utf8Bytes(text) > MAX_SETTING_TEXT_BYTES) return false;
  const lineBreaksAllowed = key === "git.message_instructions";
  for (const character of text) {
    const code = character.codePointAt(0) ?? 0;
    const control = code < 0x20 || (code >= 0x7f && code <= 0x9f);
    if (control && !(lineBreaksAllowed && (character === "\n" || character === "\t"))) return false;
  }
  return true;
}

/** An Account binding a binding or consent key can name. */
export type BindingOption = { id: string; label: string; provider?: string };

/** Each binding key and the consent key that authorizes exactly its value. */
export const CONSENT_FOR = {
  "review.account_binding": "review.cross_provider_consent",
  "utility.account_binding": "utility.content_consent",
} as const satisfies Partial<Record<SettingKeyId, SettingKeyId>>;

export type BindingKey = keyof typeof CONSENT_FOR;
export type ConsentKey = (typeof CONSENT_FOR)[BindingKey];

export function isBindingKey(key: SettingKeyId): key is BindingKey {
  return key in CONSENT_FOR;
}

export function isConsentKey(key: SettingKeyId): key is ConsentKey {
  return Object.values(CONSENT_FOR).includes(key as ConsentKey);
}

/** What an empty binding means for each binding key. */
export function emptyBindingLabel(key: BindingKey): string {
  return key === "review.account_binding" ? "Each run's own account" : "No account";
}

/**
 * Consent authorizes one exact binding UUID (docs/automatic-review.md,
 * docs/utility-work.md). Changing the binding never moves consent, so a
 * stored consent can name another account: that is `mismatch`, shown as is.
 */
export type ConsentState = "none" | "granted" | "missing" | "mismatch";

export function consentState(binding: string, consent: string): ConsentState {
  if (binding === "") return "none";
  if (consent === binding) return "granted";
  if (consent === "") return "missing";
  return "mismatch";
}

/** A binding or consent value in words: the account's label, never its UUID. */
function bindingValueText(key: SettingKeyId, id: string, bindings: ReadonlyArray<BindingOption>): string {
  const consent = isConsentKey(key);
  if (id === "") return consent ? "Not allowed" : emptyBindingLabel(key as BindingKey);
  const name = bindings.find((binding) => binding.id === id)?.label ?? "an account that's no longer connected";
  return consent ? `Allowed for ${name}` : name.charAt(0).toUpperCase() + name.slice(1);
}

/** Plain-language value for reviews and "changed elsewhere" copy. */
export function valueText(
  key: SettingKeyId,
  value: SettingValue | null,
  bindings: ReadonlyArray<BindingOption> = [],
): string {
  if (value === null) return "The inherited value";
  if (value.type === "text" && (isBindingKey(key) || isConsentKey(key))) {
    return bindingValueText(key, value.value, bindings);
  }
  switch (value.type) {
    case "flag":
      return value.value ? "On" : "Off";
    case "count": {
      const unit = SETTING_PLACEMENT[key].unit;
      return unit ? `${value.value.toLocaleString("en-US")} ${unit}` : value.value.toLocaleString("en-US");
    }
    case "text":
      return value.value === "" ? "Empty" : value.value;
    case "undisplayable":
      return "A value that can't be displayed";
  }
}

export function sameValue(left: SettingValue, right: SettingValue): boolean {
  if (left.type === "undisplayable" || right.type === "undisplayable") return left.type === right.type;
  return left.type === right.type && left.value === right.value;
}

export function sameSetting(left: ResolvedSetting, right: ResolvedSetting): boolean {
  return (
    left.key === right.key &&
    sameValue(left.value, right.value) &&
    JSON.stringify(left.source) === JSON.stringify(right.source)
  );
}

/** One row's editing state (wave 3.2 §7.2). */
export type SettingRowState =
  | { kind: "idle" }
  | { kind: "editing"; draft: SettingValue }
  | { kind: "checking"; draft: SettingValue | null }
  | { kind: "confirm"; review: SettingsReview }
  /** `draft` is the value sent; `null` sends a clear. */
  | { kind: "applying"; reviewId: string; draft: SettingValue | null }
  | { kind: "uncertain"; reviewId: string; draft: SettingValue | null; error: PublicError }
  | { kind: "changed_elsewhere"; current: ResolvedSetting; draft: SettingValue | null }
  | { kind: "refused"; error: PublicError };

/** Inline copy for refusals the Plane explains by code. */
export function refusalText(error: PublicError): string {
  switch (error.code) {
    case "setting.value_below_minimum":
      return "That's below the minimum this Plane allows.";
    case "setting.value_too_long":
      return "That's longer than this Plane allows.";
    case "security.audit_degraded":
      return "Changes to trust and policy are paused until this Plane's audit is repaired (Safety › Audit).";
    case "recovery.read_only":
      return "This Plane is in read-only recovery. Changes wait until it's restored.";
    case "protocol.feature_unavailable":
    case "protocol.unsupported_minor":
      return "This setting needs a newer Jet service on this Plane.";
    default:
      return error.message;
  }
}

/**
 * Schedules have no read or write path in `jet-client` yet (backend
 * dependency `jet_client_schedules`). The destination says so; it never
 * shows a fake list.
 */
export const schedulesAvailability = {
  kind: "dependency",
  dependency: "jet_client_schedules",
  summary: "Scheduled tasks can't be shown in this version of Jet for Linux yet.",
  detail:
    "Your Jet service supports daily schedules, but this app can't read or change them yet. Existing schedules keep running on their Plane.",
} as const;

/** Every Plane pane ends with this disclosure. */
export const LAST_WRITER_WINS =
  "Jet applies the most recent change. If another device changes the same setting at the same time, the later change wins.";

/** Why a review can't be confirmed right now, for its dialog. */
export function blockText(block: "read_only" | "stale", planeLabel: string): string {
  return block === "read_only"
    ? `${planeLabel} is in read-only recovery. Changes wait until it's restored.`
    : `Changes are paused until Jet reconnects to ${planeLabel}.`;
}
