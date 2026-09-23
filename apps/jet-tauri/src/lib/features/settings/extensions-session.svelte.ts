import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import {
  inspectExtension,
  loadExtensionCatalog,
  loadExtensionChange,
  prepareExtensionChange,
  type ExtensionAction,
  type ExtensionCatalogView,
  type ExtensionInspection,
} from "$lib/jet/extensions";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import { applySettingsChange, type SettingsReview } from "$lib/jet/settings";
import { CHANGE_POLL_MS, isTerminalChange, type ExtensionEntry, type TrackedChangeState } from "./extensions-model";
import { sectionData, sectionStateFor, withFreshness, type SectionState } from "./model";

/** Which entry an inspection or change is about. */
export type ExtensionSubject = { craftId: string; harness: string; extensionId: string };

/** The Details dialog: one entry's inspection. */
export type InspectionState =
  | { kind: "none" }
  | { kind: "loading"; subject: ExtensionSubject; entry: ExtensionEntry }
  | { kind: "ready"; subject: ExtensionSubject; entry: ExtensionEntry; inspection: ExtensionInspection }
  | { kind: "failed"; subject: ExtensionSubject; entry: ExtensionEntry; error: PublicError };

/**
 * One extension change. `confirm` shows the review; `uncertain` keeps the
 * review ID so Retry resends the same request; `queued` names the change
 * whose status is tracked.
 */
export type ExtensionOperation =
  | { kind: "idle" }
  | { kind: "preparing"; subject: ExtensionSubject; action: ExtensionAction }
  | { kind: "confirm"; subject: ExtensionSubject; action: ExtensionAction; review: SettingsReview }
  | { kind: "applying"; subject: ExtensionSubject; action: ExtensionAction; reviewId: string }
  | { kind: "uncertain"; subject: ExtensionSubject; action: ExtensionAction; reviewId: string; error: PublicError }
  | { kind: "refused"; subject: ExtensionSubject; action: ExtensionAction; error: PublicError }
  | { kind: "queued"; subject: ExtensionSubject; action: ExtensionAction; changeId: string };

/** A queued change and its last known state. */
export type TrackedChange = {
  changeId: string;
  craftId: string;
  harness: string;
  extensionId: string;
  action: ExtensionAction;
  state: TrackedChangeState;
  /** The last status read failed; the state shown is the last one known. */
  error: PublicError | null;
};

/** What the Extensions section needs from the Settings session that owns it. */
export type ExtensionsHost = {
  /** A refusal that names Plane state (degraded audit, read-only recovery). */
  planeStateStale(): void;
  /** Why changes are paused now (read-only Recovery, a stale view); `null` when they may be sent. */
  mutationBlock(): "read_only" | "stale" | null;
};

export type ExtensionsOptions = {
  /** Whether the window is visible; changes are polled only while it is. */
  visible?: () => boolean;
  pollMs?: number;
};

function documentVisible(): boolean {
  return typeof document === "undefined" || document.visibilityState !== "hidden";
}

/**
 * Agents › Extensions of one Plane: each Craft's catalog, one inspection at
 * a time, reviewed changes and the status of queued ones. Every completion
 * is checked against the Plane and request it started for.
 */
export class ExtensionsSession {
  planeId = $state<PlaneId>(LOCAL_PLANE);
  catalogs = $state<Record<string, SectionState<ExtensionCatalogView>>>({});
  inspection = $state<InspectionState>({ kind: "none" });
  operation = $state<ExtensionOperation>({ kind: "idle" });
  changes = $state<TrackedChange[]>([]);

  private host: ExtensionsHost;
  private visible: () => boolean;
  private pollMs: number;
  private started = false;
  private generation = 0;
  private catalogRequests: Record<string, number> = {};
  private inspectionRequest = 0;
  private operationRequest = 0;
  private timers = new Map<string, ReturnType<typeof setTimeout>>();
  private disposed = false;
  /** Applying and uncertain operations of Planes not shown now. */
  private retained = new Map<PlaneId, ExtensionOperation>();

  constructor(host: ExtensionsHost, options: ExtensionsOptions = {}) {
    this.host = host;
    this.visible = options.visible ?? documentVisible;
    this.pollMs = options.pollMs ?? CHANGE_POLL_MS;
  }

  /** Shows one Plane. Keeps an applying or uncertain change for when the user returns. */
  select(planeId: PlaneId): void {
    if (this.started && planeId === this.planeId) return;
    if (this.operation.kind === "applying" || this.operation.kind === "uncertain") {
      this.retained.set(this.planeId, this.operation);
    }
    this.started = true;
    this.stopPolling();
    this.generation++;
    this.planeId = planeId;
    this.catalogs = {};
    this.catalogRequests = {};
    this.inspection = { kind: "none" };
    this.changes = [];
    this.operation = this.retained.get(planeId) ?? { kind: "idle" };
    this.retained.delete(planeId);
  }

  /** Stops accepting completions and polling (the window is closing). */
  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.stopPolling();
  }

  /** The change watcher is reconnecting or stopped: catalogs show their last values as stale. */
  markStale(): void {
    this.catalogs = Object.fromEntries(
      Object.entries(this.catalogs).map(([craftId, state]) => [craftId, withFreshness(state, "stale")]),
    );
  }

  /** Reads every catalog loaded on this Plane again (the watcher resumed). */
  async reloadLoaded(): Promise<void> {
    await this.reloadAll(Object.keys(this.catalogs));
  }

  // -------------------------------------------------------------------------
  // Catalogs
  // -------------------------------------------------------------------------

  /** Loads the catalogs of Crafts not loaded yet on this Plane. */
  async ensureLoaded(craftIds: readonly string[]): Promise<void> {
    await Promise.all(craftIds.filter((id) => !(id in this.catalogs)).map((id) => this.loadCatalog(id)));
  }

  /** Reads every listed Craft's catalog again (focus, Check again). */
  async reloadAll(craftIds: readonly string[]): Promise<void> {
    await Promise.all(craftIds.map((id) => this.loadCatalog(id)));
  }

  async loadCatalog(craftId: string): Promise<void> {
    const { planeId, generation } = this;
    const request = (this.catalogRequests[craftId] ?? 0) + 1;
    this.catalogRequests[craftId] = request;
    const previous = this.catalogs[craftId];
    const last = previous ? sectionData(previous) : null;
    this.catalogs = { ...this.catalogs, [craftId]: { kind: "loading", last } };
    try {
      const catalog = await loadExtensionCatalog(planeId, craftId);
      if (generation !== this.generation || request !== this.catalogRequests[craftId]) return;
      this.catalogs = {
        ...this.catalogs,
        [craftId]: {
          kind: "ready",
          data: catalog,
          freshness: "live",
          issues: catalog.issues.map((issue) => ({ section: issue.section, error: issue.error })),
        },
      };
      // Changes queued before a navigation or Plane switch are tracked again.
      for (const change of catalog.changes) {
        this.track({
          changeId: change.changeId,
          craftId,
          harness: catalog.harness,
          extensionId: change.extensionId,
          action: change.action,
          state: "checking",
          error: null,
        });
      }
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.catalogRequests[craftId]) return;
      this.catalogs = { ...this.catalogs, [craftId]: sectionStateFor(publicError(error), last) };
    }
  }

  // -------------------------------------------------------------------------
  // Inspection
  // -------------------------------------------------------------------------

  /** Whether a new inspection or change may start. */
  get idle(): boolean {
    return !["preparing", "applying", "uncertain"].includes(this.operation.kind);
  }

  /** Opens Details for one entry: the shell inspects it by its token. */
  async inspect(craftId: string, harness: string, entry: ExtensionEntry): Promise<void> {
    if (!this.idle) return;
    const { planeId, generation } = this;
    const request = ++this.inspectionRequest;
    const subject = { craftId, harness, extensionId: entry.id };
    this.operation = { kind: "idle" };
    this.inspection = { kind: "loading", subject, entry };
    try {
      const inspection = await inspectExtension(planeId, entry.entryToken);
      if (generation !== this.generation || request !== this.inspectionRequest) return;
      this.inspection = { kind: "ready", subject, entry, inspection };
    } catch (thrown: unknown) {
      if (generation !== this.generation || request !== this.inspectionRequest) return;
      const error = publicError(thrown);
      this.inspection = { kind: "failed", subject, entry, error };
      // The entry token expired or the catalog changed: show the current list.
      if (error.code === "extensions.inspection_expired") void this.loadCatalog(craftId);
    }
  }

  /**
   * Closes Details. Not while a change is being checked or sent; an
   * uncertain or queued change stays and the section reports it.
   */
  closeInspection(): void {
    if (this.operation.kind === "preparing" || this.operation.kind === "applying") return;
    this.inspectionRequest++;
    this.inspection = { kind: "none" };
    if (this.operation.kind === "confirm" || this.operation.kind === "refused") this.operation = { kind: "idle" };
  }

  // -------------------------------------------------------------------------
  // Reviewed changes
  // -------------------------------------------------------------------------

  /** Reviews one change of the inspected entry; the user confirms the review. */
  async prepare(action: ExtensionAction): Promise<void> {
    const current = this.inspection;
    if (!this.idle || current.kind !== "ready" || !current.inspection.reviewable) return;
    const { planeId, generation } = this;
    const request = ++this.operationRequest;
    const subject = current.subject;
    this.operation = { kind: "preparing", subject, action };
    let review: SettingsReview;
    try {
      review = await prepareExtensionChange(planeId, current.inspection.inspectionId, action);
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.operationRequest) return;
      // Nothing was sent: a prepare only admits a review.
      this.operation = { kind: "refused", subject, action, error: publicError(error) };
      return;
    }
    if (generation !== this.generation || request !== this.operationRequest) return;
    this.operation = { kind: "confirm", subject, action, review };
  }

  /** Leaves the review and returns to the inspection. */
  back(): void {
    if (this.operation.kind === "confirm" || this.operation.kind === "refused") this.operation = { kind: "idle" };
  }

  /** Sends the reviewed change shown in `confirm`. */
  async confirm(): Promise<void> {
    const current = this.operation;
    if (current.kind !== "confirm") return;
    // Nothing is sent while changes are paused, even from an open review.
    if (this.host.mutationBlock() !== null) return;
    await this.apply(current.subject, current.action, current.review.reviewId);
  }

  /** Resends an uncertain change: same review ID, same body. */
  async retry(): Promise<void> {
    const current = this.operation;
    if (current.kind !== "uncertain") return;
    if (this.host.mutationBlock() !== null) return;
    await this.apply(current.subject, current.action, current.reviewId);
  }

  /** Clears a refusal or a queued notice. In-flight and uncertain changes stay. */
  dismiss(): void {
    if (this.idle) this.operation = { kind: "idle" };
  }

  private async apply(subject: ExtensionSubject, action: ExtensionAction, reviewId: string): Promise<void> {
    const { planeId } = this;
    // Any prepare still in flight is out of date.
    this.operationRequest++;
    this.operation = { kind: "applying", subject, action, reviewId };
    let receipt;
    try {
      receipt = await applySettingsChange(planeId, reviewId);
    } catch (thrown: unknown) {
      const uncertain: ExtensionOperation = { kind: "uncertain", subject, action, reviewId, error: publicError(thrown) };
      if (this.owns(planeId, reviewId)) this.operation = uncertain;
      // The user switched Planes; keep the uncertainty for when they return.
      else if (!this.disposed && planeId !== this.planeId) this.retained.set(planeId, uncertain);
      return;
    }
    if (!this.owns(planeId, reviewId)) {
      // Answered while its Plane isn't shown: a queued change is tracked
      // again from the catalog when the user returns.
      const held = this.retained.get(planeId);
      if (held && (held.kind === "applying" || held.kind === "uncertain") && held.reviewId === reviewId) {
        this.retained.delete(planeId);
      }
      return;
    }
    if (receipt.kind === "applied" && receipt.detail.kind === "extension_change_queued") {
      const { changeId } = receipt.detail;
      this.operation = { kind: "queued", subject, action, changeId };
      this.inspection = { kind: "none" };
      this.track({ changeId, ...subject, action, state: "checking", error: null });
      void this.loadCatalog(subject.craftId);
      return;
    }
    if (receipt.kind === "refused") {
      this.operation = { kind: "refused", subject, action, error: receipt.error };
      if (receipt.error.code === "security.audit_degraded" || receipt.error.code === "recovery.read_only") {
        this.host.planeStateStale();
      }
      return;
    }
    // Any other answer doesn't match an extension review.
    this.operation = { kind: "refused", subject, action, error: publicError(null) };
  }

  // -------------------------------------------------------------------------
  // Change status
  // -------------------------------------------------------------------------

  /** The tracked changes of one Craft. */
  changesFor(craftId: string): TrackedChange[] {
    return this.changes.filter((change) => change.craftId === craftId);
  }

  /** Removes a finished change from the list. */
  forget(changeId: string): void {
    const change = this.changes.find((candidate) => candidate.changeId === changeId);
    if (!change || !isTerminalChange(change.state)) return;
    this.changes = this.changes.filter((candidate) => candidate.changeId !== changeId);
  }

  private track(change: TrackedChange): void {
    if (this.changes.some((candidate) => candidate.changeId === change.changeId)) return;
    this.changes = [...this.changes, change];
    void this.poll(change.changeId, this.generation);
  }

  private update(changeId: string, patch: Partial<TrackedChange>): void {
    this.changes = this.changes.map((change) => (change.changeId === changeId ? { ...change, ...patch } : change));
  }

  private schedule(changeId: string, generation: number): void {
    const timer = setTimeout(() => {
      this.timers.delete(changeId);
      void this.poll(changeId, generation);
    }, this.pollMs);
    this.timers.set(changeId, timer);
  }

  /** Reads a change until it reaches a final state. Never retries the change. */
  private async poll(changeId: string, generation: number): Promise<void> {
    if (generation !== this.generation) return;
    const change = this.changes.find((candidate) => candidate.changeId === changeId);
    if (!change || isTerminalChange(change.state)) return;
    if (!this.visible()) {
      this.schedule(changeId, generation);
      return;
    }
    const { planeId } = this;
    try {
      const status = await loadExtensionChange(planeId, changeId);
      if (generation !== this.generation) return;
      this.update(changeId, { state: status.state, error: null });
      if (isTerminalChange(status.state)) {
        // The catalog now shows the change's result.
        void this.loadCatalog(change.craftId);
        return;
      }
    } catch (thrown: unknown) {
      if (generation !== this.generation) return;
      const error = publicError(thrown);
      this.update(changeId, { error });
      // The shell no longer knows this change: reading again can't help.
      if (error.code === "extensions.change_unknown") return;
    }
    this.schedule(changeId, generation);
  }

  /** Whether the Plane on screen still shows this review being sent. */
  private owns(planeId: PlaneId, reviewId: string): boolean {
    if (this.disposed || planeId !== this.planeId) return false;
    return this.operation.kind === "applying" && this.operation.reviewId === reviewId;
  }

  private stopPolling(): void {
    for (const timer of this.timers.values()) clearTimeout(timer);
    this.timers.clear();
  }
}
