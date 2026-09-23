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
  action: "retry" | "open_planes" | "pair_again" | null;
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
    case "identity.key_missing":
      return `This computer's pairing key for ${label} is missing. Pair again.`;
    case "plane.duplicates_local":
      return `${label} is this computer's own Plane. Forget it.`;
    case "plane.identity_changed":
      return `${label} now reaches a different Plane. Forget it and add it again if that's expected.`;
    case "ssh.connection_failed":
      return `Jet couldn't connect to ${label} over SSH. Check that \`ssh ${label}\` works in a terminal without asking questions.`;
    case "ssh.client_missing":
      return "OpenSSH isn't installed on this computer.";
    case "plane.jetd_missing":
      return `Jet isn't installed on ${label}, or \`jetd\` isn't on the \`PATH\` that SSH uses for commands.`;
    case "plane.jetd_unavailable":
      return `Jet isn't running on ${label}.`;
    case "plane.handshake_refused":
      return `${label} answered in a way Jet doesn't trust, so Jet stopped.`;
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

/**
 * A remote Plane that refused this computer's key, or whose key is gone, is
 * fixed by pairing again; this computer's own Plane never is.
 */
function pairAgainAction(
  planeId: PlaneId,
  error: PublicError,
  otherwise: RecentStatusRow["action"],
): RecentStatusRow["action"] {
  return planeId !== "local" && needsPairing(error) ? "pair_again" : otherwise;
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
          action: pairAgainAction(section.planeId, state.error, "open_planes"),
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
          action: pairAgainAction(section.planeId, state.error, "retry"),
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
      case "identity.key_missing":
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

/**
 * Display grouping for a typed authentication string: digits only, at most
 * six, shown as `ddd-ddd`. Native code validates what is sent.
 */
export function normalizeAuthString(input: string): string {
  const digits = input.replace(/[^0-9]/g, "").slice(0, 6);
  return digits.length > 3 ? `${digits.slice(0, 3)}-${digits.slice(3)}` : digits;
}

/** A key fingerprint as four lowercase groups of four hex characters. */
export function formatFingerprint(fingerprint: string): string {
  const hex = fingerprint.toLowerCase().replace(/[^0-9a-f]/g, "").slice(0, 16);
  return hex.match(/.{1,4}/g)?.join(" ") ?? "";
}

/** Digits spelled out one by one, for a code's accessible name. */
export function spokenDigits(code: string): string {
  return code.replace(/[^0-9]/g, "").split("").join(" ");
}

/** Whole milliseconds left until a decimal Unix-millisecond deadline. */
export function millisecondsLeft(expiresAtUnixMs: string, nowUnixMs: number): number {
  const deadline = Number(expiresAtUnixMs);
  return Number.isFinite(deadline) ? Math.max(0, deadline - nowUnixMs) : 0;
}

/** Plain-text countdown; it never animates and is updated coarsely. */
export function countdownText(expiresAtUnixMs: string, nowUnixMs: number): string {
  const left = millisecondsLeft(expiresAtUnixMs, nowUnixMs);
  if (left <= 0) return "Expired";
  const seconds = Math.ceil(left / 1000);
  if (seconds > 90) return `About ${Math.round(seconds / 60)} minutes left`;
  if (seconds > 60) return "About a minute left";
  return `About ${Math.max(10, Math.ceil(seconds / 10) * 10)} seconds left`;
}

export function formatClockTime(unixMs: string | number | null): string {
  const value = typeof unixMs === "string" ? Number(unixMs) : unixMs;
  if (value === null || !Number.isFinite(value)) return "soon";
  return new Date(value).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function formatPairedDate(unixMs: string): string {
  const value = Number(unixMs);
  if (!Number.isFinite(value)) return "Paired on an unknown date";
  return `Paired ${new Date(value).toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" })}`;
}

/** Copy for a Plane whose trust-changing Commands are paused. */
export function pairingPausedCopy(
  reason: "security_degraded" | "store_read_only" | null,
  label: string,
): string | null {
  switch (reason) {
    case "security_degraded":
      return `${label} can't vouch for its security record, so pairing changes are paused. Existing paired computers keep working.`;
    case "store_read_only":
      return `${label} is in read-only recovery, so pairing changes are paused until its data is restored.`;
    default:
      return null;
  }
}

export function pairingEndedCopy(
  reason: "expired" | "too_many_attempts" | "gate_closed" | "claimed",
): string {
  switch (reason) {
    case "expired":
      return "Pairing ended: the code expired.";
    case "too_many_attempts":
      return "Pairing ended: too many wrong codes.";
    case "gate_closed":
      return "Pairing ended: pairing was closed.";
    case "claimed":
      return "The code was used by another computer.";
  }
}

/**
 * Display grouping for a typed one-time code: digits only, at most eight,
 * shown as `xxxx-yyyy`. Native code validates what is sent.
 */
export function normalizeManualCode(input: string): string {
  const digits = input.replace(/[^0-9]/g, "").slice(0, 8);
  return digits.length > 4 ? `${digits.slice(0, 4)}-${digits.slice(4)}` : digits;
}

/** A refused login that pairing again can fix, so the UI offers Pair again. */
export function needsPairing(error: PublicError | null): boolean {
  return (
    error?.code === "connection.unauthorized" ||
    error?.code === "identity.key_missing" ||
    error?.code === "identity.session_ended"
  );
}

/** Only Forget helps: the entry is not a separate Plane it can reach. */
export function onlyForget(error: PublicError | null): boolean {
  return error?.code === "plane.duplicates_local" || error?.code === "plane.identity_changed";
}

/** Secure-storage problems handled by the wizard's own step (ADR-0076). */
export function secureStorageProblem(error: PublicError): boolean {
  return error.code === "identity.secret_store_unavailable" || error.code === "identity.secret_store_locked";
}

/** The Plane's code has lapsed: the wizard returns to the code step. */
export function offerLapsed(error: PublicError): boolean {
  return (
    error.code === "pairing.offer_expired" ||
    error.code === "pairing.offer_ended" ||
    error.code === "pairing.offer_superseded" ||
    error.code === "enrollment.ticket_expired"
  );
}

/**
 * Enrollment failure copy, chosen by stable code (§7.4). `existingLabel`
 * names the registered Plane an error points at, when it is still listed.
 */
export function enrollmentFailureCopy(
  error: PublicError,
  destination: string,
  existingLabel: (planeId: string) => string | null = () => null,
): string {
  switch (error.code) {
    case "ssh.connection_failed":
      return `Jet couldn't connect over SSH. Check the address, and that \`ssh ${destination}\` works in a terminal without asking questions.`;
    case "ssh.client_missing":
      return "OpenSSH isn't installed on this computer.";
    case "plane.jetd_missing":
      return `Jet isn't installed on ${destination}, or \`jetd\` isn't on the \`PATH\` that SSH uses for commands. Non-interactive SSH sessions often skip your shell profile.`;
    case "plane.jetd_unavailable":
      return `Jet isn't running on ${destination}.`;
    case "plane.handshake_refused":
      return `${destination} answered in a way Jet doesn't trust, so Jet stopped. Nothing was sent.`;
    case "pairing.secret_rejected":
      return `That code is wrong. Check it on ${destination}. After several wrong codes it stops working.`;
    case "pairing.offer_expired":
    case "pairing.offer_ended":
    case "pairing.offer_superseded":
    case "enrollment.ticket_expired":
      return `This pairing request expired. Get a new code on ${destination}.`;
    case "pairing.none_offered":
    case "pairing.gate_closed":
      return `That code is no longer valid. Get a new one on ${destination}.`;
    case "pairing.not_confirmed":
      return `${destination} hasn't confirmed yet.`;
    case "enrollment.transcript_invalid":
      return `Jet stopped pairing because ${destination}'s reply didn't check out. Nothing was paired.`;
    case "enrollment.code_invalid":
      return "Type the 8-digit code shown on the other computer.";
    case "enrollment.draft_expired":
      return "This pairing request expired. Start adding the Plane again.";
    case "plane.already_registered": {
      const existing = error.planeId ? existingLabel(error.planeId) : null;
      return existing
        ? `This is the same Plane as ${existing}.`
        : "This is the same Plane as one already on this computer.";
    }
    case "plane.identity_changed":
      return `${destination} now reaches a different Plane. Forget it and add it again if that's expected.`;
    case "plane.destination_invalid":
      return "Enter an SSH address such as user@host, or a host alias from your SSH config.";
    case "plane.limit_reached":
      return "This computer already has the maximum of 16 remote Planes. Forget one first.";
    case "connection.limit":
      return `${destination} has too many connections right now. Try again shortly.`;
    case "security.audit_degraded":
      return `${destination} isn't accepting pairing changes right now: its security record needs attention.`;
    case "recovery.read_only":
      return `${destination} isn't accepting pairing changes right now: it is in read-only recovery.`;
    case "protocol.remote_auth_required":
    case "protocol.incompatible":
    case "protocol.feature_unavailable":
      return `${destination} runs a Jet version that can't pair remotely. Update Jet there.`;
    case "identity.key_missing":
      return "This computer's pairing key is missing. Pair again.";
    case "identity.unsupported_platform":
      return "This computer has no supported secure storage for a pairing key.";
  }
  if (error.category === "incompatible") {
    return `${destination} runs a Jet version that can't pair remotely. Update Jet there.`;
  }
  return planeErrorCopy(error, destination);
}
