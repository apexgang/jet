import type { PlaneConditionKind } from "$lib/jet/errors";
import type { RunSummary } from "$lib/jet/bridge";
import type { PlaneHealthSummary } from "$lib/jet/system";
import type { SettingsTarget } from "$lib/jet/settings-window";
import { landedTarget } from "$lib/features/settings/model";

/**
 * What the main window knows about one Plane's health. The summary comes
 * only from a fresh status read (a feed's `connected` snapshot); refusals
 * observed since then add conditions until the next summary replaces them.
 */
export type PlaneConditions = {
  summary: PlaneHealthSummary | null;
  /** Conditions proven by a refusal since the last summary. */
  observed: ReadonlyArray<Exclude<PlaneConditionKind, "disk_pressure">>;
  /** When a request on this Plane was last refused for low disk space. */
  diskPressureAt: number | null;
  /** The user dismissed the disk-pressure notice. */
  dismissed: boolean;
};

export const EMPTY_CONDITIONS: PlaneConditions = {
  summary: null,
  observed: [],
  diskPressureAt: null,
  dismissed: false,
};

/** One notice at a time, most severe first (wave 3.3 §7.3). */
export const CONDITION_PRIORITY: ReadonlyArray<PlaneConditionKind> = [
  "read_only",
  "ledger_corrupt",
  "security_degraded",
  "disk_pressure",
];

/** Every condition that currently holds on a Plane. */
export function activeConditions(conditions: PlaneConditions): PlaneConditionKind[] {
  const { summary, observed } = conditions;
  const holds: Record<PlaneConditionKind, boolean> = {
    read_only: summary?.store === "read_only" || observed.includes("read_only"),
    ledger_corrupt: summary?.ledger === "corrupt" || observed.includes("ledger_corrupt"),
    security_degraded: summary?.security === "degraded" || observed.includes("security_degraded"),
    disk_pressure: conditions.diskPressureAt !== null && !conditions.dismissed,
  };
  return CONDITION_PRIORITY.filter((kind) => holds[kind]);
}

export type PlaneNotice = { kind: PlaneConditionKind; diskPressureAt: number | null };

/** The single notice to show for a Plane, or null. */
export function noticeFor(conditions: PlaneConditions): PlaneNotice | null {
  const kind = activeConditions(conditions)[0];
  return kind ? { kind, diskPressureAt: kind === "disk_pressure" ? conditions.diskPressureAt : null } : null;
}

const WHEN = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** A concrete local date and time (design-language 160). */
export function formatWhen(unixMs: number): string {
  return WHEN.format(new Date(unixMs));
}

/** The notice's sentence, naming the Plane it is about. */
export function noticeText(notice: PlaneNotice, planeLabel: string): string {
  switch (notice.kind) {
    case "read_only":
      return `${planeLabel} is in read-only recovery. Tasks can't change.`;
    case "ledger_corrupt":
      return `Jet can't verify deleted data on ${planeLabel}.`;
    case "security_degraded":
      return `Security changes and deletions are paused on ${planeLabel}.`;
    case "disk_pressure":
      return notice.diskPressureAt === null
        ? `Low disk space on ${planeLabel} stopped new work.`
        : `Low disk space on ${planeLabel} stopped new work at ${formatWhen(notice.diskPressureAt)}.`;
  }
}

/** The Settings link a notice offers, with its label, or null. */
export function noticeLink(
  kind: PlaneConditionKind,
  planeId: string,
): { label: string; target: SettingsTarget } | null {
  switch (kind) {
    case "disk_pressure": {
      const target = landedTarget("storage", planeId);
      return target ? { label: "Open Storage", target } : null;
    }
    case "security_degraded": {
      const target = landedTarget("audit", planeId);
      return target ? { label: "Review audit", target } : null;
    }
    case "read_only":
    case "ledger_corrupt":
      // "Open Recovery" needs the Safety › Recovery section, which a later
      // Wave 3.3 slice adds. Until it lands the notice offers no link.
      return null;
  }
}

/** The result line of "Free space". */
export function collectResultText(removed: number): string {
  if (removed === 0) return "Nothing to remove right now.";
  return removed === 1 ? "Removed 1 file." : `Removed ${removed.toLocaleString()} files.`;
}

/**
 * Whether the Run tab shows the Recovery needed panel: the Plane lost sight
 * of the activity's process, or supervision says it needs attention.
 */
export function needsRunRecovery(
  lifecycle: RunSummary["lifecycle"] | null | undefined,
  needsAttention: boolean | null | undefined,
): boolean {
  return lifecycle === "lost" || needsAttention === true;
}

/**
 * Lost-run disclosure. Backend dependency `jet_client_orphaned_executions`:
 * no public `jet-client` method inspects or resolves the retained process.
 */
export const LOST_RUN_TEXT =
  "Jet can't observe this activity's process. Jet keeps it attached to this task. " +
  "Inspecting and adopting or ending the retained process needs a Jet app update; it isn't available here yet.";

/** The header line for a task Jet forgets on its own; null for Retain. */
export function retentionLine(policy: "retain" | "forget_after_final_run" | null | undefined): string | null {
  return policy === "forget_after_final_run" ? "Jet forgets this task after its last activity ends." : null;
}
