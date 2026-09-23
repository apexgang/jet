import type { PublicError } from "$lib/jet/bridge";
import type {
  RetentionProtection,
  TrashEntry,
  TrashMode,
  TrashPreview,
  TrashReason,
  TrashView,
} from "$lib/jet/retention";

export { retentionLine as retentionPolicyLine } from "$lib/features/system/model";

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

/** At most this many names are resolved in one native call. */
export const NAMES_PER_CALL = 32;

const REASONS: Record<TrashReason, string> = {
  manual_forget: "You forgot it",
  automatic_forget: "Forgotten after its last activity ended",
  delete_everywhere: "You deleted it everywhere",
  autodelete_rule: "An auto-delete rule matched it",
  autodelete_everywhere: "An auto-delete rule matched it (delete everywhere)",
  plane_transfer: "Moved to another Plane",
};

export function reasonLabel(reason: TrashReason): string {
  return REASONS[reason];
}

const PROTECTIONS: Record<RetentionProtection, string> = {
  active_run: "This task has activity in progress.",
  pending_turn: "A message is waiting in the queue.",
  enabled_schedule: "A schedule will send more messages.",
  dirty_workspace: "The workspace has changes that aren't committed. They are deleted with the task.",
  unpushed_work: "The workspace has commits that aren't pushed. They are deleted with the task.",
  unresolved_effect: "A delivery or external action hasn't finished.",
};

export function protectionLine(kind: RetentionProtection): string {
  return PROTECTIONS[kind];
}

export const TOMBSTONE_TEXT = "This task now lives on another Plane. This copy can't be restored.";
export const DUE_TEXT = "Deletion is due. Jet deletes it once its remaining work ends. Restore it to keep it.";
export const CAPPED_TEXT = "Showing the 256 tasks deleted soonest. More may be in Jet Trash.";
export const SNAPSHOT_TEXT = (planeLabel: string) =>
  `Copies may remain in recovery snapshots on ${planeLabel} for up to about five weeks, or until you remove old snapshots in Settings.`;
export const UNCHECKED_TEXT =
  "Jet couldn't check this task's workspace for uncommitted or unpushed work. Anything there is deleted with the task.";
export const UNCERTAIN_TEXT =
  "Jet couldn't confirm the request. Try again to resend the same request. It won't be applied twice.";
export const STOP_ACKNOWLEDGEMENT = "Stop the activity in this task and cancel queued messages";

export type FormattedDate = { absolute: string; relative: string; iso: string };

const ABSOLUTE = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const RELATIVE = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** A concrete local date next to relative phrasing (design-language 160). */
export function formatDate(unixMs: string | number, now: number): FormattedDate {
  const ms = Number(unixMs);
  const date = new Date(ms);
  const difference = ms - now;
  const relative =
    Math.abs(difference) < DAY_MS
      ? RELATIVE.format(Math.round(difference / HOUR_MS), "hour")
      : RELATIVE.format(Math.round(difference / DAY_MS), "day");
  return { absolute: ABSOLUTE.format(date), relative, iso: date.toISOString() };
}

/** How a row reads: waiting for its date, due now, or a Transfer tombstone. */
export function trashStatus(entry: TrashEntry, now: number): "scheduled" | "due" | "tombstone" {
  if (!entry.restorable) return "tombstone";
  return Number(entry.expiresAtUnixMs) <= now ? "due" : "scheduled";
}

/** "{reason} · Moved {date} · Deleted on {date} ({relative})". */
export function rowLine(entry: TrashEntry, now: number): string {
  const moved = formatDate(entry.trashedAtUnixMs, now);
  const deleted = formatDate(entry.expiresAtUnixMs, now);
  return `${reasonLabel(entry.reason)} · Moved ${moved.absolute} · Deleted on ${deleted.absolute} (${deleted.relative})`;
}

export function bannerText(entry: TrashEntry, now: number): string {
  const deleted = formatDate(entry.expiresAtUnixMs, now);
  return `This task is in Jet Trash. Jet deletes it on ${deleted.absolute} (${deleted.relative}).`;
}

export function emptyText(view: Pick<TrashView, "planeLabel" | "graceDays">): string {
  const wait = view.graceDays === null ? "wait here" : `wait here for ${view.graceDays} ${view.graceDays === 1 ? "day" : "days"}`;
  return `Jet Trash on ${view.planeLabel} is empty. Tasks you forget or delete ${wait} before Jet deletes them.`;
}

/**
 * Task IDs the loaded task list cannot name, in chunks for one native call
 * each. IDs already known (from Recent or an earlier lookup) are skipped.
 */
export function namesToResolve(entries: readonly TrashEntry[], known: (conversationId: string) => boolean): string[][] {
  const missing = [...new Set(entries.map((entry) => entry.conversationId))].filter((id) => !known(id));
  const chunks: string[][] = [];
  for (let start = 0; start < missing.length; start += NAMES_PER_CALL) {
    chunks.push(missing.slice(start, start + NAMES_PER_CALL));
  }
  return chunks;
}

/** One Plane's Jet Trash section (wave 3.3 §7.2). */
export type Freshness = "live" | "stale";
export type TrashSectionState =
  | { kind: "loading" }
  | { kind: "ready"; view: TrashView; freshness: Freshness; loadedAt: number }
  | { kind: "empty"; view: TrashView; freshness: Freshness; loadedAt: number }
  | { kind: "offline"; last: TrashView | null }
  | { kind: "denied"; error: PublicError }
  | { kind: "unsupported"; error: PublicError }
  | { kind: "failed"; error: PublicError; last: TrashView | null };

export function sectionView(state: TrashSectionState | undefined): TrashView | null {
  if (!state) return null;
  switch (state.kind) {
    case "ready":
    case "empty":
      return state.view;
    case "offline":
    case "failed":
      return state.last;
    default:
      return null;
  }
}

export function sectionFromView(view: TrashView, loadedAt: number): TrashSectionState {
  return view.entries.length > 0
    ? { kind: "ready", view, freshness: "live", loadedAt }
    : { kind: "empty", view, freshness: "live", loadedAt };
}

/** Classifies a failed read, keeping the last trustworthy list. */
export function sectionFromError(error: PublicError, last: TrashView | null): TrashSectionState {
  if (error.category === "unauthorized") return { kind: "denied", error };
  if (error.category === "incompatible" || error.protocolLimit !== null) return { kind: "unsupported", error };
  if (error.category === "offline") return { kind: "offline", last };
  return { kind: "failed", error, last };
}

export function sectionText(state: TrashSectionState, planeLabel: string): string | null {
  switch (state.kind) {
    case "loading":
      return "Loading Jet Trash…";
    case "offline":
      return `Jet can't reach ${planeLabel}. Showing the last loaded Jet Trash. Restore is unavailable until it reconnects.`;
    case "denied":
      return `This device isn't allowed to view Jet Trash on ${planeLabel}.`;
    case "unsupported":
      return `${planeLabel} doesn't support Jet Trash. Update Jet on that Plane.`;
    case "failed":
      return `Jet couldn't load Jet Trash on ${planeLabel}. ${state.error.message}`;
    default:
      return null;
  }
}

/** A Restore that the Plane refused, in plain words (wave 3.3 §7.3). */
export function restoreRefusalText(error: Pick<PublicError, "code" | "message">, planeLabel: string): string {
  switch (error.code) {
    case "retention.not_trashed":
      return "This task is no longer in Jet Trash.";
    case "retention.transferred":
      return TOMBSTONE_TEXT;
    case "retention.trash_unknown":
      return "Jet Trash changed. Reload to see the current list.";
    case "security.audit_degraded":
      return "Jet can't restore tasks until the security audit is checked.";
    case "recovery.read_only":
      return `${planeLabel} is in read-only recovery.`;
    default:
      return error.message;
  }
}

/** Refusals after which the list is read again. */
export function restoreRefusalReloads(error: Pick<PublicError, "code">): boolean {
  return ["retention.not_trashed", "retention.transferred", "retention.trash_unknown"].includes(error.code);
}

/** A Move to Trash refusal, in plain words. */
export function moveRefusalText(error: Pick<PublicError, "code" | "message">): string {
  switch (error.code) {
    case "retention.live_work":
      return "Jet won't forget a task while it has activity in progress or queued messages. Use Stop Run or withdraw the messages, or choose Delete everywhere.";
    case "retention.run_starting":
      return "Activity is starting in this task. Try again in a moment.";
    case "retention.review_stale":
      return "Activity started in this task after you opened this dialog. Review again to see what stops.";
    case "retention.request_unresolved":
      return "Jet is still confirming an earlier request for this task. Try that one again first.";
    case "retention.already_trashed":
      return "This task is already in Jet Trash.";
    case "retention.review_expired":
      return "This review expired. Review the task again.";
    default:
      return error.message;
  }
}

export function modeName(mode: TrashMode): string {
  return mode === "forget" ? "Forget in Jet" : "Delete everywhere";
}

/** "about {date}" from the grace period; the Plane sets the real date. */
export function estimatedDeletion(graceDays: number | null, now: number): string | null {
  if (graceDays === null) return null;
  return ABSOLUTE_DAY.format(new Date(now + graceDays * DAY_MS));
}

const ABSOLUTE_DAY = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

/** The consequence line of each mode (wave 3.3 §7.3). */
export function modeConsequence(mode: TrashMode, preview: Pick<TrashPreview, "graceDays">, now: number): string {
  const estimate = estimatedDeletion(preview.graceDays, now);
  const when = estimate ? `on about ${estimate}` : "after the Plane's grace period";
  const until = estimate ? `until about ${estimate}` : "until then";
  if (mode === "forget") {
    return `Jet deletes its copy of this task ${when}. The Harness keeps its own history. You can restore the task until then.`;
  }
  return (
    `Jet deletes its copy ${when} and records a request to delete the Harness's history too. ` +
    "No installed Harness supports this yet; Jet records the request. " +
    "Any activity in this task stops now and queued messages are canceled. " +
    `You can restore the task ${until}, but stopped work doesn't restart.`
  );
}

export function auditLine(records: string | null): string | null {
  if (records === null || records === "0") return null;
  return records === "1"
    ? "1 security audit record mentions this task. It stays, without its contents, until it expires."
    : `${records} security audit records mention this task. They stay, without its contents, until they expire.`;
}

/** The status line under a row or the banner after Restore, or null. */
export function restoreActionText(
  action:
    | { kind: "idle" | "restoring" | "restored" }
    | { kind: "refused" | "uncertain"; error: Pick<PublicError, "code" | "message"> },
  planeLabel: string,
): string | null {
  switch (action.kind) {
    case "idle":
      return null;
    case "restoring":
      return "Restoring…";
    case "restored":
      return "Restored. Refreshing Jet Trash…";
    case "refused":
      return restoreRefusalText(action.error, planeLabel);
    case "uncertain":
      return "Jet couldn't confirm the restore. Try again to resend the same request. It won't be applied twice.";
  }
}

/** The Settings link a refusal offers, labelled for its section. */
export function refusalLinkLabel(section: string | null | undefined, paneTitle: string): string {
  switch (section) {
    case "audit":
      return "Open Security audit";
    case "storage":
      return "Open Storage";
    default:
      return `Open ${paneTitle} settings`;
  }
}
