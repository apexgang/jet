import {
  loadConversations,
  searchConversations,
  type ConversationPage,
  type ConversationRow,
  type PublicError,
} from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import type { PlaneId } from "$lib/jet/planes";

import {
  MAXIMUM_CHAIN_RESTARTS,
  RECENT_WINDOW,
  REMOTE_PAGE_LIMIT,
  mergeRecent,
  recentStatusRows,
  retainNewest,
  searchFailure,
  searchGroups,
  sectionData,
  sectionFromError,
  type CatalogSection,
  type RecentRow,
  type SearchPlaneState,
} from "./model";
import type { PlanesSession } from "./session.svelte";

export type SearchState = {
  text: string;
  request: number;
  byPlane: Record<PlaneId, SearchPlaneState>;
};

/** Events that change existing rows, so the whole chain is re-walked. */
const REWALK_EVENTS = new Set([
  "conversation.name_changed",
  "conversation.trashed",
  "conversation.restored",
]);

/** A new task only appends, so the chain is re-read from its tail page. */
const TAIL_EVENT = "conversation.created";

const REFRESH_DEBOUNCE_MS = 1_000;

/**
 * Recent and Search across every registered Plane. Each Plane keeps its own
 * page chain, feed fence, freshness and error, so one failing Plane never
 * blanks another. Rows are merged newest-first only for display.
 *
 * Conversation pages come oldest-first, so each chain is walked to its end and
 * only the newest rows are retained. The real fix is a newest-first query in
 * the backend (`recent_conversations_query`).
 */
export class PlaneCatalog {
  sections = $state<Record<PlaneId, CatalogSection>>({});
  visibleLimit = $state(RECENT_WINDOW);
  searchText = $state("");
  searchState = $state<SearchState | null>(null);
  /** Optional Plane filter; the Search group headings toggle it. */
  searchFilter = $state<PlaneId | null>(null);

  private generations = new Map<PlaneId, number>();
  private walks = new Map<PlaneId, Promise<void>>();
  private refreshTimers = new Map<PlaneId, { full: boolean; timer: ReturnType<typeof setTimeout> }>();
  private searchRequest = 0;

  private readonly planes: PlanesSession;

  constructor(planes: PlanesSession) {
    this.planes = planes;
  }

  /** Sections in registry order: this computer first. */
  get orderedSections(): CatalogSection[] {
    return this.planes.planes
      .map((plane) => this.sections[plane.planeId])
      .filter((section): section is CatalogSection => section !== undefined);
  }

  rows = $derived.by((): RecentRow[] =>
    mergeRecent(this.orderedSections).map((row) => ({
      ...row,
      planeLabel: this.planes.label(row.planeId),
    })),
  );

  get visibleRows(): RecentRow[] {
    return this.rows.slice(0, this.visibleLimit);
  }

  get hasMore(): boolean {
    return this.rows.length > this.visibleLimit;
  }

  showMore(): void {
    this.visibleLimit += RECENT_WINDOW;
  }

  get statusRows() {
    return recentStatusRows(this.orderedSections, (planeId) => this.planes.label(planeId));
  }

  /** Every section has settled into ready and holds no rows. */
  get empty(): boolean {
    const sections = this.orderedSections;
    return (
      sections.length > 0 &&
      sections.every((section) => section.state.kind === "ready") &&
      this.rows.length === 0
    );
  }

  section(planeId: PlaneId): CatalogSection | null {
    return this.sections[planeId] ?? null;
  }

  cursor(planeId: PlaneId): string | null {
    return this.sections[planeId]?.cursor ?? null;
  }

  find(planeId: PlaneId, conversationId: string): ConversationRow | null {
    return (
      sectionData(this.sections[planeId]?.state)?.find((row) => row.id === conversationId) ?? null
    );
  }

  /**
   * Starts a walk for every registered Plane without a section and drops the
   * sections of Planes that are no longer registered.
   */
  sync(): void {
    const registered = new Set(this.planes.planes.map((plane) => plane.planeId));
    for (const planeId of Object.keys(this.sections)) {
      if (!registered.has(planeId)) this.forget(planeId);
    }
    for (const planeId of registered) {
      if (!this.sections[planeId] && !this.walks.has(planeId)) void this.load(planeId);
    }
  }

  /** Resolves when the latest walk of a Plane has settled. */
  settled(planeId: PlaneId): Promise<void> {
    return this.walks.get(planeId) ?? Promise.resolve();
  }

  /** Drops a Plane's section; late pages from it are ignored. */
  forget(planeId: PlaneId): void {
    this.bump(planeId);
    this.walks.delete(planeId);
    const pending = this.refreshTimers.get(planeId);
    if (pending) clearTimeout(pending.timer);
    this.refreshTimers.delete(planeId);
    const { [planeId]: _dropped, ...rest } = this.sections;
    this.sections = rest;
    if (this.searchState && planeId in this.searchState.byPlane) {
      const { [planeId]: _hit, ...byPlane } = this.searchState.byPlane;
      this.searchState = { ...this.searchState, byPlane };
    }
    if (this.searchFilter === planeId) this.searchFilter = null;
  }

  /** (Re)walks one Plane's chain. Only that Plane's section changes. */
  load(planeId: PlaneId): Promise<void> {
    const walk = this.walk(planeId);
    this.walks.set(planeId, walk);
    return walk;
  }

  private bump(planeId: PlaneId): number {
    const generation = (this.generations.get(planeId) ?? 0) + 1;
    this.generations.set(planeId, generation);
    return generation;
  }

  private update(planeId: PlaneId, change: Partial<CatalogSection>): void {
    const previous = this.sections[planeId] ?? this.initial(planeId);
    this.sections = { ...this.sections, [planeId]: { ...previous, ...change } };
  }

  private initial(planeId: PlaneId): CatalogSection {
    return {
      planeId,
      planeIdentity: this.planes.plane(planeId)?.planeIdentity ?? null,
      state: { kind: "loading" },
      cursor: null,
      tailPage: null,
      pages: 0,
      chain: "idle",
      partial: null,
      restarts: 0,
      savedAtUnixMs: null,
    };
  }

  private async walk(planeId: PlaneId): Promise<void> {
    const generation = this.bump(planeId);
    const current = () => this.generations.get(planeId) === generation && this.planes.has(planeId);
    if (!current()) return;
    const previous = sectionData(this.sections[planeId]?.state);
    this.update(planeId, {
      state: previous ? this.sections[planeId].state : { kind: "loading" },
      chain: "paging",
    });
    const pageLimit = this.planes.plane(planeId)?.kind === "remote" ? REMOTE_PAGE_LIMIT : Infinity;
    let restarts = 0;
    let gathered: ConversationRow[] = [];
    for (;;) {
      let rows: ConversationRow[] = [];
      let pages = 0;
      let tailPage: string | null = null;
      try {
        let page: ConversationPage = await loadConversations(null, planeId);
        if (!current()) return;
        const cursor = page.cursor;
        pages = 1;
        rows = retainNewest(rows, page.conversations);
        // The first page's cursor fences this Plane's feed.
        this.update(planeId, { cursor });
        void this.planes.ensureFeed(planeId, cursor);
        let partial: CatalogSection["partial"] = null;
        while (page.nextPage) {
          if (pages >= pageLimit) {
            partial = "page_limit";
            break;
          }
          tailPage = page.nextPage;
          page = await loadConversations(tailPage, planeId);
          if (!current()) return;
          pages += 1;
          rows = retainNewest(rows, page.conversations);
        }
        this.update(planeId, {
          planeIdentity: this.planes.plane(planeId)?.planeIdentity ?? null,
          state: { kind: "ready", data: rows },
          tailPage,
          pages,
          chain: partial ? "partial" : "complete",
          partial,
          restarts,
          savedAtUnixMs: Date.now(),
        });
        return;
      } catch (error: unknown) {
        if (!current()) return;
        const failure = publicError(error);
        if (failure.restart?.reason === "pagination_stale" && pages > 0) {
          // The Plane changed under the walk: restart this Plane's chain only.
          gathered = retainNewest(gathered, rows);
          restarts += 1;
          if (restarts < MAXIMUM_CHAIN_RESTARTS) continue;
          this.update(planeId, {
            state: { kind: "ready", data: retainNewest(previous ?? [], gathered) },
            tailPage,
            pages,
            chain: "partial",
            partial: "restarts",
            restarts,
            savedAtUnixMs: Date.now(),
          });
          return;
        }
        this.fail(planeId, failure, previous);
        // Without a first page there is no fence yet. Open the feed anyway so
        // its native reconnect loop reports when the Plane is back; the
        // section is re-walked on that `connected` update.
        if (pages === 0) void this.planes.ensureFeed(planeId, null);
        return;
      }
    }
  }

  private fail(planeId: PlaneId, error: PublicError, data: ConversationRow[] | null): void {
    this.update(planeId, { state: sectionFromError(error, data), chain: "idle" });
  }

  /** A Plane's feed reported a failure: keep its rows and mark them stale. */
  markUnavailable(planeId: PlaneId, error: PublicError): void {
    const section = this.sections[planeId];
    if (!section || section.chain === "paging") return;
    this.fail(planeId, error, sectionData(section.state));
  }

  /** A Plane's feed (re)connected: rebuild it if it was not current. */
  reconnected(planeId: PlaneId): void {
    const section = this.sections[planeId];
    if (!section || section.chain === "paging") return;
    if (section.state.kind !== "ready") void this.load(planeId);
  }

  /**
   * Feed events on Plane P refresh only P's section, debounced. A created task
   * re-reads from P's `tailPage`; a rename or trash re-walks P's whole chain.
   * A pending re-walk absorbs a later tail read, never the other way round.
   */
  receiveEvent(planeId: PlaneId, kind: string): void {
    const rewalk = REWALK_EVENTS.has(kind);
    if ((!rewalk && kind !== TAIL_EVENT) || !this.sections[planeId]) return;
    const pending = this.refreshTimers.get(planeId);
    if (pending) clearTimeout(pending.timer);
    const full = rewalk || (pending?.full ?? false);
    this.refreshTimers.set(planeId, {
      full,
      timer: setTimeout(() => {
        this.refreshTimers.delete(planeId);
        void (full ? this.load(planeId) : this.loadTail(planeId));
      }, REFRESH_DEBOUNCE_MS),
    });
  }

  /**
   * Re-reads one Plane's chain from its `tailPage`, which answers the last
   * page plus anything newer, and merges the rows into the section. Falls back
   * to a full walk when there is no tail yet, a walk is running, or the Plane
   * reports that its pagination went stale.
   */
  loadTail(planeId: PlaneId): Promise<void> {
    const section = this.sections[planeId];
    const existing = sectionData(section?.state);
    if (!section || !section.tailPage || section.chain === "paging" || section.state.kind !== "ready" || !existing) {
      return this.load(planeId);
    }
    const read = this.readTail(planeId, section, existing);
    this.walks.set(planeId, read);
    return read;
  }

  private async readTail(planeId: PlaneId, section: CatalogSection, existing: ConversationRow[]): Promise<void> {
    const generation = this.bump(planeId);
    const current = () => this.generations.get(planeId) === generation && this.planes.has(planeId);
    const pageLimit = this.planes.plane(planeId)?.kind === "remote" ? REMOTE_PAGE_LIMIT : Infinity;
    let tailPage = section.tailPage;
    // The tail page is read again, so it is not counted twice.
    let pages = Math.max(section.pages - 1, 0);
    let rows = existing;
    let partial: CatalogSection["partial"] = section.partial;
    this.update(planeId, { chain: "paging" });
    try {
      let page: ConversationPage = await loadConversations(tailPage, planeId);
      if (!current()) return;
      pages += 1;
      rows = retainNewest(rows, page.conversations);
      while (page.nextPage) {
        if (pages >= pageLimit) {
          partial = "page_limit";
          break;
        }
        tailPage = page.nextPage;
        page = await loadConversations(tailPage, planeId);
        if (!current()) return;
        pages += 1;
        rows = retainNewest(rows, page.conversations);
      }
      this.update(planeId, {
        state: { kind: "ready", data: rows },
        tailPage,
        pages,
        chain: partial ? "partial" : "complete",
        partial,
        savedAtUnixMs: Date.now(),
      });
    } catch (error: unknown) {
      if (!current()) return;
      const failure = publicError(error);
      if (failure.restart?.reason === "pagination_stale") {
        await this.load(planeId);
        return;
      }
      this.fail(planeId, failure, rows);
    }
  }

  /** Adds or replaces one row the shell learned about directly. */
  upsert(row: ConversationRow): void {
    const section = this.sections[row.planeId];
    const rows = sectionData(section?.state);
    if (!section || !rows) return;
    const merged = retainNewest(rows, [row]);
    this.update(row.planeId, {
      state: section.state.kind === "stale" ? { ...section.state, data: merged } : { kind: "ready", data: merged },
    });
  }

  /**
   * One search per Plane, grouped by Plane in each Plane's own rank order.
   * A Plane that failed for good shows its failure instead of being asked.
   */
  async search(text = this.searchText): Promise<void> {
    const request = ++this.searchRequest;
    const trimmed = text.trim();
    if (!trimmed) {
      this.searchState = null;
      return;
    }
    const byPlane: Record<PlaneId, SearchPlaneState> = {};
    const targets: PlaneId[] = [];
    for (const plane of this.planes.planes) {
      if (plane.connection.state === "failed" && !plane.connection.error.retryable) {
        byPlane[plane.planeId] = searchFailure(plane.connection.error);
      } else {
        byPlane[plane.planeId] = { kind: "searching" };
        targets.push(plane.planeId);
      }
    }
    this.searchState = { text: trimmed, request, byPlane };
    await Promise.all(
      targets.map(async (planeId) => {
        let state: SearchPlaneState;
        try {
          state = { kind: "ready", result: await searchConversations(trimmed, planeId) };
        } catch (error: unknown) {
          state = searchFailure(publicError(error));
        }
        if (request !== this.searchRequest || !this.searchState || !this.planes.has(planeId)) return;
        this.searchState = {
          ...this.searchState,
          byPlane: { ...this.searchState.byPlane, [planeId]: state },
        };
      }),
    );
  }

  get searching(): boolean {
    return Object.values(this.searchState?.byPlane ?? {}).some((state) => state.kind === "searching");
  }

  get groups() {
    return this.searchState
      ? searchGroups(this.searchState.byPlane, this.planes.planes, this.searchFilter)
      : [];
  }

  toggleSearchFilter(planeId: PlaneId): void {
    this.searchFilter = this.searchFilter === planeId ? null : planeId;
  }

  /** Clears pending refresh timers, for example when the window closes. */
  dispose(): void {
    for (const pending of this.refreshTimers.values()) clearTimeout(pending.timer);
    this.refreshTimers.clear();
    for (const planeId of this.generations.keys()) this.bump(planeId);
  }
}
