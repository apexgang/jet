import type { ConversationRow, ConversationSearchResult, PublicError } from "$lib/jet/bridge";
import type { FeatureName, FeatureSupport, Plane, PlaneId, ProtocolKnowledge } from "$lib/jet/planes";

/** One Plane-scoped section of the UI. Every failure keeps its own error. */
export type Section<T> =
  | { kind: "loading" }
  | { kind: "ready"; data: T }
  | { kind: "stale"; data: T; error: PublicError }
  | { kind: "offline"; error: PublicError }
  | { kind: "denied"; error: PublicError }
  | { kind: "unsupported"; error: PublicError }
  | { kind: "failed"; error: PublicError };

export type SectionFailureKind = Exclude<Section<unknown>["kind"], "loading" | "ready">;

/** One Plane's Recent chain. Cursors and failures never mix across Planes. */
export type CatalogSection = {
  planeId: PlaneId;
  planeIdentity: string | null;
  state: Section<ConversationRow[]>;
  /** The first page's cursor: this Plane's feed fence. */
  cursor: string | null;
  /** The page token that produced the last page of the latest walk. */
  tailPage: string | null;
  pages: number;
  chain: "idle" | "paging" | "complete" | "partial";
  partial: "page_limit" | "restarts" | null;
  restarts: number;
  /** Wall-clock time of the last complete or partial walk, for "saved {time}". */
  savedAtUnixMs: number | null;
};

export type RecentRow = ConversationRow & { planeLabel: string };

export type RecentStatusKind =
  | "loading"
  | "offline"
  | "stale"
  | "failed"
  | "denied"
  | "unsupported"
  | "partial";

export type RecentStatusRow = {
  planeId: PlaneId;
  label: string;
  kind: RecentStatusKind;
  error: PublicError | null;
  text: string;
  action: "retry" | "open_planes" | null;
};

export type SearchPlaneState =
  | { kind: "searching" }
  | { kind: "ready"; result: ConversationSearchResult }
  | { kind: "unsupported"; error: PublicError }
  | { kind: "offline" | "failed"; error: PublicError };

export type SearchGroup = {
  planeId: PlaneId;
  label: string;
  showHeading: boolean;
  state: SearchPlaneState;
  /** Copy shown instead of, or next to, the hits. */
  text: string | null;
  indexing: boolean;
};

export const RECENT_RETAINED_PER_PLANE = 4096;
export const REMOTE_PAGE_LIMIT = 256;
export const RECENT_WINDOW = 200;
export const MAXIMUM_CHAIN_RESTARTS = 3;

const DECIMAL = /^(0|[1-9][0-9]*)$/;

/**
 * Compares canonical decimal strings exactly. Timestamps and sequences can
 * exceed 2^53, so they never go through `Number`. A malformed value sorts
 * below every valid one.
 */
export function compareDecimal(a: string, b: string): number {
  const validA = DECIMAL.test(a);
  const validB = DECIMAL.test(b);
  if (!validA || !validB) return validA === validB ? 0 : validA ? 1 : -1;
  const left = BigInt(a);
  const right = BigInt(b);
  return left === right ? 0 : left > right ? 1 : -1;
}

function compareText(a: string, b: string): number {
  return a === b ? 0 : a < b ? -1 : 1;
}

/** Newest first by `(createdAtUnixMs, id)`. */
function newestFirst(a: ConversationRow, b: ConversationRow): number {
  return compareDecimal(b.createdAtUnixMs, a.createdAtUnixMs) || compareText(b.id, a.id);
}

/**
 * Merges one page into a Plane's retained rows and keeps the `limit` rows with
 * the greatest `(createdAtUnixMs, id)`. A row seen again replaces the older
 * copy, so a rename is picked up.
 */
export function retainNewest(
  rows: readonly ConversationRow[],
  incoming: readonly ConversationRow[],
  limit = RECENT_RETAINED_PER_PLANE,
): ConversationRow[] {
  const byId = new Map<string, ConversationRow>();
  for (const row of rows) byId.set(row.id, row);
  for (const row of incoming) byId.set(row.id, row);
  return [...byId.values()].sort(newestFirst).slice(0, Math.max(0, limit));
}

export function sectionData<T>(section: Section<T> | undefined): T | null {
  return section && (section.kind === "ready" || section.kind === "stale") ? section.data : null;
}

/**
 * Recent across Planes: `createdAtUnixMs` descending, then Plane identity
 * (the Plane handle while the identity is unknown), then Conversation ID.
 * Timestamps order the display only (ADR-0069); nothing else depends on them.
 */
export function mergeRecent(sections: readonly CatalogSection[]): ConversationRow[] {
  const keyed: Array<{ row: ConversationRow; plane: string }> = [];
  for (const section of sections) {
    const rows = sectionData(section.state);
    if (!rows) continue;
    const plane = section.planeIdentity ?? section.planeId;
    for (const row of rows) keyed.push({ row, plane });
  }
  keyed.sort(
    (a, b) =>
      compareDecimal(b.row.createdAtUnixMs, a.row.createdAtUnixMs) ||
      compareText(a.plane, b.plane) ||
      compareText(a.row.id, b.row.id),
  );
  return keyed.map(({ row }) => row);
}

/**
 * The Section kind an error puts a Plane-scoped read in.
 * - offline, or retryable unavailable → offline (stale with data);
 * - unauthorized → denied;
 * - incompatible → unsupported;
 * - anything else, including non-retryable unavailable → failed (stale with data).
 */
export function classifySection(error: PublicError, hasData: boolean): SectionFailureKind {
  if (error.category === "unauthorized") return "denied";
  if (error.category === "incompatible") return "unsupported";
  if (hasData) return "stale";
  if (error.category === "offline" || (error.category === "unavailable" && error.retryable)) {
    return "offline";
  }
  return "failed";
}

export function sectionFromError<T>(error: PublicError, data: T | null): Section<T> {
  const kind = classifySection(error, data !== null);
  if (kind === "stale") return { kind, data: data as T, error };
  return { kind, error };
}

function isOfflineError(error: PublicError): boolean {
  return error.category === "offline" || (error.category === "unavailable" && error.retryable);
}

/**
 * Plane-named copy for an error, chosen by stable code first and category
 * second. Protocol numbers never appear here (they live in the feature table).
 */
export function planeErrorCopy(error: PublicError, label: string): string {
  switch (error.code) {
    case "identity.secret_store_locked":
      return `Unlock your keyring to reach ${label}.`;
    case "connection.unauthorized":
      return `This computer isn't allowed on ${label}.`;
    case "identity.session_ended":
      return `This computer paired with ${label} for one session only. Pair again to reconnect.`;
    case "plane.review_moved":
      return `${label} changed since this was prepared. Review it again.`;
    case "security.audit_degraded":
      return `${label} can't vouch for its security record. Changes to trust are paused.`;
    case "recovery.read_only":
      return `${label} is in read-only recovery. Changes are paused until its data is restored.`;
  }
  if (error.category === "incompatible") return `Not available on ${label}. Update Jet on ${label}.`;
  if (error.category === "unauthorized") return `This computer isn't allowed on ${label}.`;
  if (isOfflineError(error)) return `${label} is offline.`;
  return error.message;
}

export function formatSavedTime(unixMs: number | null): string {
  if (unixMs === null) return "earlier";
  return new Date(unixMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** One status row for every Plane whose Recent section is not simply ready. */
export function recentStatusRows(
  sections: readonly CatalogSection[],
  labelOf: (planeId: PlaneId) => string,
): RecentStatusRow[] {
  const rows: RecentStatusRow[] = [];
  for (const section of sections) {
    const label = labelOf(section.planeId);
    const base = { planeId: section.planeId, label };
    const state = section.state;
    switch (state.kind) {
      case "loading":
        rows.push({ ...base, kind: "loading", error: null, text: `Loading tasks from ${label}…`, action: null });
        break;
      case "ready":
        if (section.chain === "partial") {
          rows.push({
            ...base,
            kind: "partial",
            error: null,
            text: `Newest tasks from ${label} may be missing.`,
            action: "open_planes",
          });
        }
        break;
      case "stale": {
        const saved = `Showing tasks saved ${formatSavedTime(section.savedAtUnixMs)}.`;
        const cause = isOfflineError(state.error)
          ? planeErrorCopy(state.error, label)
          : `Tasks from ${label} couldn't refresh.`;
        rows.push({ ...base, kind: "stale", error: state.error, text: `${cause} ${saved}`, action: "retry" });
        break;
      }
      case "offline":
        rows.push({
          ...base,
          kind: "offline",
          error: state.error,
          text: planeErrorCopy(state.error, label),
          action: "retry",
        });
        break;
      case "denied":
        rows.push({
          ...base,
          kind: "denied",
          error: state.error,
          text: `${label} doesn't allow this computer.`,
          action: "open_planes",
        });
        break;
      case "unsupported":
        rows.push({
          ...base,
          kind: "unsupported",
          error: state.error,
          text: `Tasks from ${label} need a newer Jet. Update Jet on ${label}.`,
          action: "open_planes",
        });
        break;
      case "failed":
        rows.push({
          ...base,
          kind: "failed",
          error: state.error,
          text: `Tasks from ${label} couldn't load.`,
          action: "retry",
        });
        break;
    }
  }
  return rows;
}

/** The Search state a failed per-Plane search is shown in. */
export function searchFailure(error: PublicError): SearchPlaneState {
  if (error.category === "incompatible") return { kind: "unsupported", error };
  if (isOfflineError(error)) return { kind: "offline", error };
  return { kind: "failed", error };
}

/**
 * Search groups: local first, then registry order. Each group keeps its
 * Plane's own hit order; hits are never interleaved across Planes. Headings
 * are hidden while only one Plane is registered.
 */
export function searchGroups(
  byPlane: Readonly<Record<PlaneId, SearchPlaneState>>,
  planes: readonly Pick<Plane, "planeId" | "label">[],
  filter: PlaneId | null = null,
): SearchGroup[] {
  const showHeading = planes.length > 1;
  const groups: SearchGroup[] = [];
  for (const plane of planes) {
    const state = byPlane[plane.planeId];
    if (!state || (filter !== null && filter !== plane.planeId)) continue;
    const label = plane.label;
    let text: string | null = null;
    let indexing = false;
    switch (state.kind) {
      case "searching":
        text = `Searching ${label}…`;
        break;
      case "ready":
        indexing = compareDecimal(state.result.indexedThrough, state.result.cursor) < 0;
        if (state.result.hits.length === 0) {
          text = showHeading ? `No matches on ${label}` : "No matching tasks";
        }
        break;
      case "unsupported":
        text = `Search isn't available on ${label}. Update Jet on ${label}.`;
        break;
      case "offline":
        text = isOfflineError(state.error) ? `${label} is offline` : planeErrorCopy(state.error, label);
        break;
      case "failed":
        text = planeErrorCopy(state.error, label);
        break;
    }
    groups.push({ planeId: plane.planeId, label, showHeading, state, text, indexing });
  }
  return groups;
}

const FEATURE_NAMES: Record<FeatureName, string> = {
  remote_login: "Remote sign-in",
  pairing: "Pairing",
  capabilities: "Capabilities",
  conversation_pages: "Task list paging",
  projects: "Projects",
  workspaces: "Workspaces",
  search: "Search",
  run_supervision: "Run supervision",
  workspace_terminals: "Workspace terminals",
  approval_retry: "Approval retry",
  git_delivery: "Git delivery",
};

export function featureName(feature: FeatureName): string {
  return FEATURE_NAMES[feature] ?? feature;
}

/**
 * Feature-table status. This table is the only place protocol numbers appear.
 */
export function featureLabel(feature: FeatureSupport, protocol: ProtocolKnowledge): string {
  switch (feature.support) {
    case "supported":
      return "Available";
    case "unsupported": {
      const needs = `Needs Jet protocol 1.${feature.requiredMinor}`;
      if (protocol.exact !== null) return `${needs} (this Plane: 1.${protocol.exact})`;
      if (protocol.atMost !== null) return `${needs} (this Plane: 1.${protocol.atMost} or older)`;
      return needs;
    }
    default:
      return "Checked when used";
  }
}

export type PlaneStatusTone = "ok" | "progress" | "warning" | "danger";

export type PlaneStatus = { text: string; tone: PlaneStatusTone; attention: boolean };

/**
 * One text state per Plane for lists and badges. Status is never conveyed by
 * colour alone: `text` always names it.
 */
export function planeStatus(plane: Pick<Plane, "connection" | "security" | "store">): PlaneStatus {
  const connection = plane.connection;
  if (connection.state === "failed") {
    switch (connection.error.code) {
      case "connection.unauthorized":
        return { text: "Needs pairing", tone: "danger", attention: true };
      case "identity.session_ended":
        return { text: "Session ended", tone: "danger", attention: true };
      default:
        return { text: "Unavailable", tone: "danger", attention: true };
    }
  }
  if (plane.security === "degraded") {
    return { text: "Security needs attention", tone: "warning", attention: true };
  }
  if (plane.store === "read_only") return { text: "Read-only", tone: "warning", attention: true };
  switch (connection.state) {
    case "online":
      return { text: "Connected", tone: "ok", attention: false };
    case "connecting":
      return { text: "Connecting", tone: "progress", attention: false };
    case "reconnecting":
      return { text: "Reconnecting", tone: "warning", attention: false };
    default:
      return { text: "Not connected", tone: "progress", attention: false };
  }
}

export function connectionLabel(plane: Pick<Plane, "connection" | "security" | "store">): string {
  return planeStatus(plane).text;
}

/**
 * Planes that need attention: failed, needs pairing, session ended,
 * Security-degraded, read-only store, or a claim waiting for confirmation.
 */
export function planeAttentionCount(
  planes: readonly Pick<Plane, "planeId" | "connection" | "security" | "store">[],
  pairingByPlane: Readonly<Record<PlaneId, { awaitingConfirmation: boolean }>> = {},
): number {
  return planes.filter(
    (plane) => planeStatus(plane).attention || pairingByPlane[plane.planeId]?.awaitingConfirmation === true,
  ).length;
}

/** The sidebar's aggregate Plane status block. */
export function aggregateStatus(
  planes: readonly Pick<Plane, "label" | "connection" | "security" | "store">[],
): { title: string; detail: string; tone: PlaneStatusTone } {
  if (planes.length === 1) {
    const status = planeStatus(planes[0]);
    return { title: planes[0].label, detail: status.text, tone: status.tone };
  }
  const connected = planes.filter((plane) => plane.connection.state === "online").length;
  const tone: PlaneStatusTone = planes.some((plane) => planeStatus(plane).attention)
    ? "warning"
    : connected === planes.length
      ? "ok"
      : "progress";
  return { title: "Planes", detail: `${connected} of ${planes.length} Planes connected`, tone };
}

/** "Plane {8-hex}" prefix of a Plane identity, for detail views only. */
export function identityPrefix(identity: string | null): string | null {
  if (!identity) return null;
  const hex = identity.replace(/-/g, "").slice(0, 8);
  return /^[0-9a-f]{8}$/i.test(hex) ? hex.toLowerCase() : null;
}
