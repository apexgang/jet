import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import { collectDisposableStorage, loadSystemHealth, type SystemHealth } from "$lib/jet/system";
import { sectionData, sectionStateFor, withFreshness, type SectionState } from "$lib/features/settings/model";
import { withRecentCode } from "./model";

/** "Free disposable space" in Settings › Safety › Storage. */
export type CollectState =
  | { kind: "idle" }
  | { kind: "collecting" }
  | { kind: "done"; removed: number }
  | { kind: "failed"; error: PublicError };

/** Setting keys whose value the health read discloses. */
const DISCLOSED_KEYS: ReadonlySet<string> = new Set(["storage.disposable_mib", "retention.trash_grace_days"]);

/**
 * The Settings window's view of one Plane's health: versions and
 * capabilities, storage and the diagnostic summary (wave 3.3 §7.2). It
 * follows the Settings Plane picker. Every completion is checked against
 * the Plane and the request it started for, and a new Jet service start
 * on the Plane invalidates everything read before it.
 */
export class SystemSession {
  planeId = $state<PlaneId>(LOCAL_PLANE);
  health = $state<SectionState<SystemHealth>>({ kind: "loading", last: null });
  collect = $state<CollectState>({ kind: "idle" });
  /** When this window last saw a request on this Plane refused for low disk space. */
  diskPressureAt = $state<number | null>(null);
  /** Stable error codes this window saw, oldest first, at most 20. */
  recentCodes = $state<string[]>([]);
  /** Counts Plane selections, so a shown pane loads again after each one. */
  selection = $state(0);

  private started = false;
  private loaded = false;
  private disposed = false;
  /** Bumped on a Plane switch and on a new Jet service start. */
  private generation = 0;
  private request = 0;
  /** The Plane's daemon start count from the last health read. */
  private daemonStarts: string | null = null;
  private now: () => number;
  /** Told when a health read shows the Plane's Jet service started again. */
  private onRestart: () => void;

  constructor(now: () => number = Date.now, onRestart: () => void = () => undefined) {
    this.now = now;
    this.onRestart = onRestart;
  }

  /** Shows one Plane. Nothing is loaded until the Safety pane asks. */
  select(planeId: PlaneId): void {
    if (this.started && planeId === this.planeId) return;
    this.started = true;
    this.loaded = false;
    this.generation++;
    this.planeId = planeId;
    this.health = { kind: "loading", last: null };
    this.collect = { kind: "idle" };
    this.diskPressureAt = null;
    this.daemonStarts = null;
    this.selection++;
  }

  /** Stops accepting completions (the window is closing). */
  dispose(): void {
    this.disposed = true;
    this.generation++;
  }

  /** The Settings change watcher is reconnecting or stopped. */
  markStale(): void {
    this.health = withFreshness(this.health, "stale");
  }

  /** Loads health once; later loads come from Check again and Plane events. */
  async ensureLoaded(): Promise<void> {
    if (this.loaded) return;
    this.loaded = true;
    await this.load();
  }

  /** Reloads when the Safety pane has loaded this Plane (watcher resumed, audit epoch). */
  async reloadIfLoaded(): Promise<void> {
    if (!this.loaded) return;
    await this.load();
  }

  /** A Plane-scope Setting changed; the disclosed budget or grace period may be out of date. */
  async settingChanged(key: string | null): Promise<void> {
    if (key === null || DISCLOSED_KEYS.has(key)) await this.reloadIfLoaded();
  }

  /**
   * Reads health. `fresh` is Check again: the Plane observes its
   * capabilities anew instead of answering with the last observation.
   */
  async load(fresh = false): Promise<void> {
    const { planeId } = this;
    let generation = this.generation;
    const request = ++this.request;
    const last = sectionData(this.health);
    this.health = { kind: "loading", last };
    try {
      const health = await loadSystemHealth(planeId, fresh);
      if (this.disposed || generation !== this.generation || request !== this.request) return;
      if (this.daemonStarts !== null && health.service.daemonStarts !== this.daemonStarts) {
        // The Plane restarted, possibly from an older store: nothing read
        // before this start is trusted any more.
        generation = this.restarted();
      }
      this.daemonStarts = health.service.daemonStarts;
      if (generation !== this.generation) return;
      for (const issue of health.issues) this.observe(issue.error);
      this.health = {
        kind: "ready",
        data: health,
        freshness: "live",
        issues: health.issues.map((issue) => ({ section: issue.section, error: issue.error })),
      };
    } catch (thrown: unknown) {
      if (this.disposed || generation !== this.generation || request !== this.request) return;
      const error = publicError(thrown);
      this.observe(error);
      this.health = sectionStateFor(error, last);
    }
  }

  /** "Free disposable space": one bounded pass on this Plane. */
  async freeSpace(): Promise<void> {
    if (this.collect.kind === "collecting") return;
    const { planeId, generation } = this;
    this.collect = { kind: "collecting" };
    try {
      const { removed } = await collectDisposableStorage(planeId);
      if (this.disposed || generation !== this.generation) return;
      this.collect = { kind: "done", removed };
    } catch (thrown: unknown) {
      if (this.disposed || generation !== this.generation) return;
      const error = publicError(thrown);
      this.observe(error);
      this.collect = { kind: "failed", error };
    }
  }

  /**
   * Records a failure this window saw on the selected Plane: its stable
   * code for the diagnostic summary, and the time of a low-disk refusal.
   */
  observe(error: PublicError): void {
    if (error.planeId !== null && error.planeId !== this.planeId) return;
    this.recentCodes = withRecentCode(this.recentCodes, error);
    if (error.code === "storage.disk_pressure") this.diskPressureAt = this.now();
  }

  /** A new Jet service start: drop what the old one reported. */
  private restarted(): number {
    this.generation++;
    this.collect = { kind: "idle" };
    this.onRestart();
    return this.generation;
  }
}
