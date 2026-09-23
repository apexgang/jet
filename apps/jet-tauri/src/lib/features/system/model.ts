import type { PlaneConditionKind } from "$lib/jet/errors";
import type { RunSummary } from "$lib/jet/bridge";
import type { PublicError } from "$lib/jet/bridge";
import type { ProtocolKnowledge } from "$lib/jet/planes";
import type {
  PlaneHealthSummary,
  RecoveryReview,
  RecoveryView,
  SecurityView,
  SnapshotReason,
  SystemHealth,
} from "$lib/jet/system";
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
    case "ledger_corrupt": {
      // Null until the Safety › Recovery section lands.
      const target = landedTarget("recovery", planeId);
      return target ? { label: "Open Recovery", target } : null;
    }
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

// ---------------------------------------------------------------------------
// Settings › Safety and system: versions, storage and diagnostics
// ---------------------------------------------------------------------------

/** Stable error codes kept for the diagnostic summary. */
export const RECENT_CODES_LIMIT = 20;

const STABLE_CODE = /^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$/;

/** Whether a string is a stable error code (no free text, no identifiers). */
export function isStableCode(code: string): boolean {
  return code.length <= 64 && STABLE_CODE.test(code);
}

/** Appends a code, keeping the newest `RECENT_CODES_LIMIT`, oldest first. */
export function withRecentCode(codes: readonly string[], error: Pick<PublicError, "code">): string[] {
  if (!isStableCode(error.code)) return [...codes];
  return [...codes, error.code].slice(-RECENT_CODES_LIMIT);
}

/** The negotiated protocol as far as this app can prove it. */
export function negotiatedLine(protocol: ProtocolKnowledge): string | null {
  if (protocol.exact !== null) return `Connected with protocol 1.${protocol.exact}`;
  return null;
}

export function credentialStoreText(store: SystemHealth["credentialStore"]): string {
  if (store === null) return "Secure storage status unavailable";
  switch (store.state) {
    case "available":
      return "Secure storage ready";
    case "locked":
      return "Secure storage locked. Unlock it in your desktop session.";
    case "unavailable":
      return "Secure storage unavailable";
  }
}

export function toolText(tool: SystemHealth["tools"][number]): string {
  return tool.version === null ? "Not installed" : tool.version;
}

/** "Started 12 times", counted by the Plane since its store was created. */
export function startsText(daemonStarts: string): string {
  return daemonStarts === "1" ? "Jet has started once on this Plane." : `Jet has started ${daemonStarts} times on this Plane.`;
}

/** Parses a decimal Unix-millisecond string the shell sent. */
export function unixMs(value: string): number | null {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

/** The Storage section's line about the last low-disk refusal, if any. */
export function diskPressureText(at: number | null): string | null {
  return at === null
    ? null
    : `Jet refused new work for low disk space at ${formatWhen(at)}. Free up disk space, then try again.`;
}

export function budgetText(disposableMiB: number | null): string | null {
  return disposableMiB === null ? null : `Temporary file budget: ${disposableMiB.toLocaleString("en-US")} MiB.`;
}

// Diagnostic summary --------------------------------------------------------

const VERSION = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,47}$/;
const PLATFORM_PART = /^[a-z0-9_]{1,32}$/;

function version(value: string): string {
  return VERSION.test(value) ? value : "unrecognized";
}

function digits(value: string): string {
  return /^[0-9]{1,20}$/.test(value) ? value : "unrecognized";
}

function platform(value: string | null): string {
  if (value === null) return "not reported";
  const parts = value.split(" · ");
  return parts.length === 2 && parts.every((part) => PLATFORM_PART.test(part)) ? `${parts[0]} ${parts[1]}` : "unrecognized";
}

function protocolSummary(protocol: ProtocolKnowledge): string {
  if (protocol.exact !== null) return `1.${protocol.exact}`;
  const most = protocol.atMost === null ? "" : `, at most 1.${protocol.atMost}`;
  return `at least 1.${protocol.atLeast}${most}`;
}

function securitySummary(security: SecurityView): string {
  switch (security.kind) {
    case "trusted":
      return "trusted";
    case "absent":
      return "not reported";
    case "degraded":
      return `degraded (${security.breach}, epoch ${digits(security.epoch)})`;
  }
}

function recoverySummary(health: SystemHealth): string[] {
  const { recovery } = health;
  switch (recovery.kind) {
    case "unsupported":
      return ["Store: recovery not reported"];
    case "serving":
    case "read_only": {
      const state = recovery.kind === "serving" ? "serving" : `read-only (${recovery.reason})`;
      const ledger = recovery.ledger.kind === "verified" ? `verified, ${digits(recovery.ledger.deletions)} deletions` : recovery.ledger.kind;
      return [`Store: ${state}, ${recovery.snapshotCount} recovery snapshots`, `Deletion ledger: ${ledger}`];
    }
  }
}

/**
 * A copyable summary for support. Every line comes from an allowlisted
 * field or enum: no Plane label, ID, path, prompt, Craft name or native
 * version line. Free-form versions that are not plain version numbers are
 * replaced by "unrecognized".
 */
export function diagnosticSummary(health: SystemHealth | null, recentCodes: readonly string[]): string {
  const lines = ["Jet for Linux diagnostic summary"];
  if (health === null) {
    lines.push("Plane health: not loaded");
  } else {
    lines.push(
      `App version: ${version(health.app.version)}`,
      `Supported protocol: ${version(health.app.supportedProtocol)}`,
      `Negotiated protocol: ${protocolSummary(health.protocol)}`,
      `Jet service version: ${version(health.service.coreVersion)}`,
      `Jet service starts: ${digits(health.service.daemonStarts)}`,
      `Platform: ${platform(health.platform)}`,
    );
    if (health.tools.length > 0) {
      lines.push(`Tools: ${health.tools.map((tool) => `${tool.tool} ${tool.version === null ? "missing" : "present"}`).join(", ")}`);
    }
    lines.push(
      `Crafts installed: ${health.crafts.length}`,
      `Secure storage: ${health.credentialStore?.state ?? "not reported"}`,
      `Degraded: ${health.degraded.length === 0 ? "none" : health.degraded.map((condition) => condition.kind).join(", ")}`,
      ...recoverySummary(health),
      `Security audit: ${securitySummary(health.security)}`,
      `Temporary file budget: ${health.storage.disposableMiB === null ? "not read" : `${health.storage.disposableMiB} MiB`}`,
      `Jet Trash grace period: ${health.retention.graceDays === null ? "not read" : `${health.retention.graceDays} days`}`,
    );
    for (const issue of health.issues) {
      if (isStableCode(issue.error.code)) lines.push(`Not loaded: ${issue.section} (${issue.error.code})`);
    }
  }
  const codes = recentCodes.filter(isStableCode).slice(-RECENT_CODES_LIMIT);
  lines.push(`Recent error codes, oldest first: ${codes.length === 0 ? "none" : codes.join(", ")}`);
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Settings › Safety and system › Recovery
// ---------------------------------------------------------------------------

const SNAPSHOT_REASONS: Record<SnapshotReason, string> = {
  daily: "Daily",
  migration: "Before an update",
  maintenance: "Before maintenance",
};

export function snapshotReasonLabel(reason: SnapshotReason): string {
  return SNAPSHOT_REASONS[reason];
}

const BYTE_UNITS = ["bytes", "KB", "MB", "GB", "TB"] as const;

/** A decimal byte count the shell sent, in the largest fitting unit. */
export function bytesText(bytes: string): string {
  const value = Number(bytes);
  if (!/^[0-9]{1,20}$/.test(bytes) || !Number.isFinite(value)) return "Size unknown";
  let scaled = value;
  let unit = 0;
  while (scaled >= 1000 && unit < BYTE_UNITS.length - 1) {
    scaled /= 1000;
    unit++;
  }
  if (unit === 0) return value === 1 ? "1 byte" : `${value.toLocaleString("en-US")} bytes`;
  return `${scaled.toLocaleString("en-US", { maximumFractionDigits: scaled < 10 ? 1 : 0 })} ${BYTE_UNITS[unit]}`;
}

/** A snapshot's date from its decimal Unix-millisecond stamp. */
export function snapshotWhen(takenAtUnixMs: string): string {
  const at = unixMs(takenAtUnixMs);
  return at === null ? "an unknown date" : formatWhen(at);
}

export const LEDGER_CORRUPT_TEXT =
  "Jet can't verify its record of deleted data, so it won't restore a snapshot. Deleted tasks could come back. " +
  "Contact support with a diagnostic summary.";

export const EXPORT_BUNDLE_TEXT = "Exporting a portable Recovery bundle isn't available in this app yet.";

function snapshotsCount(count: number): string {
  return count === 1 ? "1 verified recovery snapshot" : `${count.toLocaleString("en-US")} verified recovery snapshots`;
}

/** The Recovery section's first line for a Plane. */
export function recoveryHeadline(recovery: RecoveryView, planeLabel: string): string {
  switch (recovery.kind) {
    case "unsupported":
      return `${planeLabel} doesn't report recovery snapshots to this app. Update Jet on that Plane.`;
    case "serving":
      return `Your Jet data on ${planeLabel} is healthy. Jet keeps ${snapshotsCount(recovery.snapshotCount)}.`;
    case "read_only": {
      const reason =
        recovery.reason === "integrity_check_failed"
          ? " (the data check failed)"
          : recovery.reason === "migration_failed"
            ? " (an update couldn't finish)"
            : "";
      return (
        `Jet found a problem with its data on ${planeLabel}${reason}. ` +
        "You can read tasks, but nothing can change until you restore a snapshot."
      );
    }
  }
}

/** Why a restore can't be offered for a read-only Plane, or null when it can. */
export function restoreBlock(recovery: RecoveryView): string | null {
  if (recovery.kind !== "read_only") return null;
  if (recovery.ledger.kind === "corrupt") return LEDGER_CORRUPT_TEXT;
  if (recovery.snapshots.length === 0) {
    return "Jet has no verified snapshot to restore. Contact support with a diagnostic summary.";
  }
  return null;
}

/** Why a purge can't be offered, or null when it can. */
export function purgeBlock(recovery: RecoveryView, security: SecurityView, planeLabel: string): string | null {
  if (recovery.kind !== "serving") return null;
  if (recovery.ledger.kind === "corrupt") return LEDGER_CORRUPT_TEXT;
  if (recovery.ledger.kind === "unsupported") {
    return `${planeLabel} can't remove old snapshots from this app. Update Jet on that Plane.`;
  }
  if (security.kind !== "trusted") {
    return "Removing old snapshots waits until the security audit on this Plane is checked.";
  }
  if (recovery.snapshotCount === 0) return "There are no snapshots to remove.";
  return null;
}

/** The lines of a restore review, in reading order. */
export function restoreReviewLines(review: Extract<RecoveryReview, { kind: "restore_snapshot" }>): string[] {
  const when = snapshotWhen(review.takenAtUnixMs);
  return [
    `Changes made on ${review.planeLabel} after ${when} are replaced.`,
    "Jet keeps the damaged data beside the store as a file. It isn't deleted.",
    "Tasks and accounts deleted after the snapshot stay deleted.",
    "If the snapshot is older than the newest security record, Jet will ask you to review the security audit afterwards.",
  ];
}

/** The consequence line of a purge review. */
export function purgeReviewText(review: Extract<RecoveryReview, { kind: "purge_snapshots" }>): string {
  const count = review.snapshotCount === 1 ? "the 1 snapshot" : `all ${review.snapshotCount.toLocaleString("en-US")}`;
  const rollback = review.includesRollback ? ", including rollback copies for the previous version" : "";
  return (
    "Remove snapshots that may still contain deleted tasks? Jet takes a new snapshot now and then removes older " +
    `ones, possibly ${count} (${bytesText(review.totalBytes)})${rollback}. Removed snapshots can't be recovered.`
  );
}

export function purgedText(removedCount: number): string {
  if (removedCount === 0) return "Jet took a new snapshot. No older snapshot needed removing.";
  return removedCount === 1
    ? "Jet took a new snapshot and removed 1 older one."
    : `Jet took a new snapshot and removed ${removedCount.toLocaleString("en-US")} older ones.`;
}

/**
 * Local preconditions and review-state refusals: the view the dialog was
 * opened from is out of date, and Reload is the way on.
 */
const STALE_RECOVERY_CODES: ReadonlySet<string> = new Set([
  "recovery.snapshot_gone",
  "recovery.not_read_only_local",
  "recovery.not_read_only",
  "recovery.purge_unavailable",
  "recovery.deletion_ledger_corrupt",
  "recovery.request_unresolved",
  "recovery.review_expired",
  "client.review_used",
  "client.review_plane_mismatch",
  "plane.review_moved",
]);

export function isStaleRecoveryError(error: Pick<PublicError, "code">): boolean {
  return STALE_RECOVERY_CODES.has(error.code);
}

/** Plain copy for a refused or stale restore or purge. */
export function recoveryRefusalText(error: Pick<PublicError, "code" | "message">, planeLabel: string): string {
  switch (error.code) {
    case "recovery.not_read_only":
    case "recovery.not_read_only_local":
      return `${planeLabel} isn't in read-only recovery now. Reload to see its current state.`;
    case "recovery.snapshot_gone":
      return "This snapshot is no longer listed. Reload to see the current snapshots.";
    case "recovery.deletion_ledger_corrupt":
      return LEDGER_CORRUPT_TEXT;
    case "recovery.restore_failed":
      return `The restored snapshot didn't pass Jet's checks, so ${planeLabel} is still in read-only recovery. Try another snapshot.`;
    case "recovery.purge_unavailable":
      return "Old snapshots can't be removed right now. Reload to check the data, its record of deleted data and the security audit.";
    case "recovery.read_only":
      return `${planeLabel} is in read-only recovery. Restore a snapshot first.`;
    case "security.audit_degraded":
      return "Jet can't remove snapshots until the security audit is checked.";
    case "recovery.request_unresolved":
      return "Jet is still confirming an earlier recovery request. Reload to check its state.";
    case "recovery.review_expired":
      return "This review expired. Review again.";
    case "client.review_used":
      return "This review was already used. Reload to see what happened, then review again.";
    default:
      return error.message;
  }
}

/** After an unconfirmed restore or purge, what the re-read Plane shows. */
export type UnconfirmedCheck = "checking" | "serving" | "read_only" | "unknown";

export function unconfirmedText(review: RecoveryReview, check: UnconfirmedCheck): string {
  if (review.kind === "purge_snapshots") {
    return check === "checking"
      ? "Jet couldn't confirm whether old snapshots were removed. Checking the current state…"
      : "Jet couldn't confirm whether old snapshots were removed. The list shows what the Plane reports now.";
  }
  switch (check) {
    case "checking":
      return "Jet couldn't confirm the restore. Checking the current state…";
    case "serving":
      return "The store is serving again. The restore likely completed.";
    case "read_only":
      return "The Plane is still read-only. Review a snapshot again to retry.";
    case "unknown":
      return "Jet couldn't check the Plane's current state. Reload Recovery when it's reachable.";
  }
}
