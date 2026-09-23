import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import {
  collectDisposableStorage,
  executeRecoveryAction,
  loadSystemHealth,
  prepareRecoveryAction,
  type RecoveryAction,
  type RecoveryOutcome,
  type RecoveryReview,
  type SystemHealth,
} from "$lib/jet/system";
import { sectionData, sectionStateFor, withFreshness, type SectionState } from "$lib/features/settings/model";
import { AuditSession } from "./audit.svelte";
import { isStaleRecoveryError, withRecentCode, type UnconfirmedCheck } from "./model";

/** What the Recovery section reviews: a snapshot restore or a purge. */
export type SnapshotAction = Exclude<RecoveryAction, { kind: "begin_audit_epoch" }>;
export type SnapshotReview = Exclude<RecoveryReview, { kind: "begin_audit_epoch" }>;

/** "Free disposable space" in Settings › Safety › Storage. */
export type CollectState =
  | { kind: "idle" }
  | { kind: "collecting" }
  | { kind: "done"; removed: number }
  | { kind: "failed"; error: PublicError };

/**
 * The Recovery section's review dialog (wave 3.3 §7.2). A restore or purge is
 * sent at most once: `unconfirmed` re-reads the Plane and never resends.
 */
export type RecoveryDialog =
  | { kind: "closed" }
  | { kind: "preparing"; action: SnapshotAction["kind"] }
  | { kind: "review"; review: SnapshotReview }
  | { kind: "sending"; review: SnapshotReview }
  | { kind: "done"; outcome: Extract<RecoveryOutcome, { kind: "restored" | "purged" }>; planeLabel: string }
  | { kind: "unconfirmed"; review: SnapshotReview; error: PublicError; check: UnconfirmedCheck }
  | { kind: "stale"; error: PublicError }
  | { kind: "refused"; error: PublicError }
  /** Jet on the Plane started again while a review was open; review again. */
  | { kind: "restarted" };

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
  /** Restore or purge review in Settings › Safety › Recovery. */
  recovery = $state<RecoveryDialog>({ kind: "closed" });
  /** Settings › Safety › Audit: records, evidence export and a new audit period. */
  readonly audit: AuditSession = new AuditSession({
    observe: (error) => this.observe(error),
    onExported: () => void this.load(),
    securityChanged: () => void this.auditEpochBegun(),
  });

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
  /** Told after a restore replaced the Plane's store: reload everything shown. */
  private onRestored: () => void;
  /** Bumped when the recovery dialog closes or the Plane changes. */
  private dialogRequest = 0;

  constructor(
    now: () => number = Date.now,
    onRestart: () => void = () => undefined,
    onRestored: () => void = () => undefined,
  ) {
    this.now = now;
    this.onRestart = onRestart;
    this.onRestored = onRestored;
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
    this.dialogRequest++;
    this.recovery = { kind: "closed" };
    this.audit.select(planeId);
    this.selection++;
  }

  /** Stops accepting completions (the window is closing). */
  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.dialogRequest++;
    this.audit.dispose();
  }

  /**
   * The Plane's audit began a new period (`audit.epoch_begun`, or this
   * window's own request): the Security state and the records changed.
   */
  async auditEpochBegun(): Promise<void> {
    await Promise.all([this.reloadIfLoaded(), this.audit.reloadIfLoaded()]);
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

  // Recovery ------------------------------------------------------------------

  /**
   * Asks the shell to review a restore of one listed snapshot, or a purge.
   * The shell re-reads the Plane first; a local precondition that no longer
   * holds comes back as `stale`.
   */
  async prepareRecovery(action: SnapshotAction): Promise<void> {
    if (this.recovery.kind === "preparing" || this.recovery.kind === "sending") return;
    const { planeId } = this;
    const request = ++this.dialogRequest;
    this.recovery = { kind: "preparing", action: action.kind };
    try {
      const review = await prepareRecoveryAction(planeId, action);
      if (!this.current(request, planeId)) return;
      if (review.kind === "begin_audit_epoch") {
        // Not a snapshot review: never offered from Recovery.
        this.recovery = { kind: "closed" };
        return;
      }
      this.recovery = { kind: "review", review };
    } catch (thrown: unknown) {
      if (!this.current(request, planeId)) return;
      const error = publicError(thrown);
      this.observe(error);
      this.recovery = isStaleRecoveryError(error) ? { kind: "stale", error } : { kind: "refused", error };
    }
  }

  /** Sends the reviewed restore or purge once. */
  async confirmRecovery(): Promise<void> {
    if (this.recovery.kind !== "review") return;
    const { review } = this.recovery;
    const { planeId } = this;
    const request = this.dialogRequest;
    this.recovery = { kind: "sending", review };
    let outcome: RecoveryOutcome;
    try {
      outcome = await executeRecoveryAction(planeId, review.reviewId);
    } catch (thrown: unknown) {
      // The shell records every outcome it reaches; a thrown error means it
      // could not say what happened, which is the same as unconfirmed.
      outcome = { kind: "unconfirmed", error: publicError(thrown) };
    }
    if (!this.current(request, planeId)) return;
    switch (outcome.kind) {
      case "restored":
        // The store moved backwards: nothing read from the old one is kept.
        this.daemonStarts = null;
        this.restarted();
        this.recovery = { kind: "done", outcome, planeLabel: review.planeLabel };
        this.onRestored();
        await this.load();
        return;
      case "purged":
        this.recovery = { kind: "done", outcome, planeLabel: review.planeLabel };
        await this.load();
        return;
      case "epoch_begun":
        // Only the Audit section sends a new audit period.
        this.recovery = { kind: "closed" };
        await this.auditEpochBegun();
        return;
      case "refused":
        this.observe(outcome.error);
        this.recovery = isStaleRecoveryError(outcome.error)
          ? { kind: "stale", error: outcome.error }
          : { kind: "refused", error: outcome.error };
        await this.load();
        return;
      case "unconfirmed": {
        this.observe(outcome.error);
        this.recovery = { kind: "unconfirmed", review, error: outcome.error, check: "checking" };
        await this.load();
        if (!this.current(request, planeId) || this.recovery.kind !== "unconfirmed") return;
        const check = this.unconfirmedCheck();
        this.recovery = { ...this.recovery, check };
        // The restore most likely replaced the store: reload everything shown.
        if (review.kind === "restore_snapshot" && check === "serving") this.onRestored();
        return;
      }
    }
  }

  /** Closes the dialog; a review being sent stays until its outcome. */
  closeRecovery(): void {
    if (this.recovery.kind === "sending") return;
    this.dialogRequest++;
    this.recovery = { kind: "closed" };
  }

  /** Reload from a stale or refused dialog: close it and read the Plane again. */
  async reloadRecovery(): Promise<void> {
    this.closeRecovery();
    await this.load();
  }

  private current(request: number, planeId: PlaneId): boolean {
    return !this.disposed && request === this.dialogRequest && planeId === this.planeId;
  }

  private unconfirmedCheck(): UnconfirmedCheck {
    if (this.health.kind !== "ready") return "unknown";
    switch (this.health.data.recovery.kind) {
      case "serving":
        return "serving";
      case "read_only":
        return "read_only";
      case "unsupported":
        return "unknown";
    }
  }

  /** A new Jet service start: drop what the old one reported. */
  private restarted(): number {
    this.generation++;
    this.collect = { kind: "idle" };
    // A review read before the start may name a store that is gone.
    if (this.recovery.kind === "review" || this.recovery.kind === "preparing") {
      this.dialogRequest++;
      this.recovery = { kind: "restarted" };
    }
    void this.audit.restarted();
    this.onRestart();
    return this.generation;
  }
}
