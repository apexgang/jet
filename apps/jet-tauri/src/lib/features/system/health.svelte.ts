import type { ConnectionSnapshot, PublicError } from "$lib/jet/bridge";
import { planeConditionFor, publicError } from "$lib/jet/errors";
import type { PlaneId } from "$lib/jet/planes";
import { collectDisposableStorage } from "$lib/jet/system";
import { EMPTY_CONDITIONS, noticeFor, type PlaneConditions, type PlaneNotice } from "./model";

export type CollectState =
  | { kind: "idle" }
  | { kind: "collecting" }
  | { kind: "done"; removed: number }
  | { kind: "failed"; error: PublicError };

/**
 * Per-Plane health conditions for the main window (wave 3.3 §7.2, §7.5). A
 * condition on one Plane never shows for a task on another. Summaries come
 * only from fresh status reads; refusals add conditions in between.
 */
export class PlaneHealth {
  conditions = $state<Record<PlaneId, PlaneConditions>>({});
  collects = $state<Record<PlaneId, CollectState>>({});
  /** Set to move focus to the Plane-health notice; the notice clears it. */
  focusPending = $state(false);

  private daemonStarts = new Map<PlaneId, string>();
  private generations = new Map<PlaneId, number>();
  private readonly now: () => number;

  constructor(now: () => number = Date.now) {
    this.now = now;
  }

  conditionsOf(planeId: PlaneId): PlaneConditions {
    return this.conditions[planeId] ?? EMPTY_CONDITIONS;
  }

  notice(planeId: PlaneId): PlaneNotice | null {
    return noticeFor(this.conditionsOf(planeId));
  }

  collectState(planeId: PlaneId): CollectState {
    return this.collects[planeId] ?? { kind: "idle" };
  }

  /**
   * Applies a fresh status read from a feed snapshot. Returns true when the
   * Plane's daemon started again since the last one: its store may have been
   * replaced by an older snapshot, so every Plane cache must be dropped.
   */
  applyConnection(planeId: PlaneId, connection: Pick<ConnectionSnapshot, "state" | "health" | "daemonStarts">): boolean {
    if (connection.state !== "online") return false;
    const previous = this.daemonStarts.get(planeId);
    const starts = connection.daemonStarts;
    const restarted = previous !== undefined && starts !== null && starts !== previous;
    if (starts !== null) this.daemonStarts.set(planeId, starts);
    if (restarted) this.reset(planeId);
    this.update(planeId, { summary: connection.health, observed: [] });
    return restarted;
  }

  /** Drops everything known about a Plane after its daemon restarted. */
  reset(planeId: PlaneId): void {
    this.bump(planeId);
    this.conditions[planeId] = EMPTY_CONDITIONS;
    this.collects[planeId] = { kind: "idle" };
  }

  /** The Plane's feed dropped: late completions for it are ignored. */
  offline(planeId: PlaneId): void {
    this.bump(planeId);
    if (this.collectState(planeId).kind === "collecting") this.collects[planeId] = { kind: "idle" };
  }

  /** Records what a failed request on `planeId` proves about the Plane. */
  observe(planeId: PlaneId, error: Pick<PublicError, "code">): void {
    const kind = planeConditionFor(error);
    if (!kind) return;
    const current = this.conditionsOf(planeId);
    if (kind === "disk_pressure") {
      this.update(planeId, { diskPressureAt: this.now(), dismissed: false });
    } else if (!current.observed.includes(kind)) {
      this.update(planeId, { observed: [...current.observed, kind] });
    }
  }

  /** A request on `planeId` was admitted: the disk-pressure refusal is over. */
  succeeded(planeId: PlaneId): void {
    if (this.conditionsOf(planeId).diskPressureAt === null) return;
    this.update(planeId, { diskPressureAt: null, dismissed: false });
  }

  /**
   * A new audit epoch began: the Security-degraded condition is cleared
   * until the next status read says otherwise.
   */
  clearSecurity(planeId: PlaneId): void {
    const current = this.conditionsOf(planeId);
    this.update(planeId, {
      summary: current.summary?.security === "degraded" ? { ...current.summary, security: "unknown" } : current.summary,
      observed: current.observed.filter((kind) => kind !== "security_degraded"),
    });
  }

  /** Hides the disk-pressure notice until the next refusal. */
  dismiss(planeId: PlaneId): void {
    if (this.conditionsOf(planeId).diskPressureAt !== null) this.update(planeId, { dismissed: true });
  }

  /** Asks the view to focus the notice (Needs attention). */
  focusNotice(): void {
    this.focusPending = true;
  }

  /** "Free space": one bounded collection of disposable storage on a Plane. */
  async collect(planeId: PlaneId): Promise<void> {
    if (this.collectState(planeId).kind === "collecting") return;
    const generation = this.generation(planeId);
    this.collects[planeId] = { kind: "collecting" };
    try {
      const { removed } = await collectDisposableStorage(planeId);
      if (generation !== this.generation(planeId)) return;
      this.collects[planeId] = { kind: "done", removed };
    } catch (error: unknown) {
      if (generation !== this.generation(planeId)) return;
      const failure = publicError(error);
      this.observe(planeId, failure);
      this.collects[planeId] = { kind: "failed", error: failure };
    }
  }

  private update(planeId: PlaneId, change: Partial<PlaneConditions>): void {
    this.conditions[planeId] = { ...this.conditionsOf(planeId), ...change };
  }

  private generation(planeId: PlaneId): number {
    return this.generations.get(planeId) ?? 0;
  }

  private bump(planeId: PlaneId): void {
    this.generations.set(planeId, this.generation(planeId) + 1);
  }
}
