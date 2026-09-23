import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import {
  executeRecoveryAction,
  exportSecurityAudit,
  loadSecurityAudit,
  prepareRecoveryAction,
  type AuditEntry,
  type RecoveryReview,
} from "$lib/jet/system";
import { isStaleEpochError } from "./audit-model";

/** Records kept in view at once; beyond this, save the evidence instead. */
export const MAX_SHOWN_RECORDS = 4096;

/**
 * The Security audit viewer (wave 3.3 §7.2). `loading` keeps the records
 * already shown while newer ones load; offline and failed reads keep them.
 */
export type AuditState =
  | { kind: "idle" }
  | { kind: "loading"; entries: AuditEntry[] }
  | { kind: "ready"; entries: AuditEntry[]; next: string | null; complete: boolean }
  | { kind: "denied"; error: PublicError }
  | { kind: "unsupported"; error: PublicError }
  | { kind: "offline"; entries: AuditEntry[]; error: PublicError }
  | { kind: "failed"; entries: AuditEntry[]; error: PublicError };

/** "Save audit evidence…". The chosen path stays in the shell. */
export type ExportState =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved"; records: string; fileName: string }
  | { kind: "failed"; error: PublicError };

export type EpochReview = Extract<RecoveryReview, { kind: "begin_audit_epoch" }>;

/**
 * "Start new audit period…". The epoch Command is deduplicated by its
 * receipt, so an uncertain send becomes `retry_epoch`, whose "Try again"
 * resends the same review.
 */
export type EpochDialog =
  | { kind: "closed" }
  | { kind: "preparing" }
  | { kind: "review"; review: EpochReview }
  | { kind: "sending"; review: EpochReview }
  | { kind: "done"; epoch: string; planeLabel: string }
  | { kind: "retry_epoch"; review: EpochReview; error: PublicError }
  | { kind: "stale"; error: PublicError }
  | { kind: "refused"; error: PublicError }
  /** Jet on the Plane started again while a review was open; review again. */
  | { kind: "restarted" };

export type AuditHooks = {
  /** Records a failure's stable code for the diagnostic summary. */
  observe: (error: PublicError) => void;
  /** Evidence was saved: the health read now shows it. */
  onExported: () => void;
  /** The Security state may have changed (a new audit period, a stale review): read it again. */
  securityChanged: () => void;
};

/**
 * One Plane's Security audit in Settings › Safety › Audit: pages oldest
 * first, evidence export and the new-audit-period review. Every completion
 * is checked against the Plane and the request it started for.
 */
export class AuditSession {
  planeId = $state<PlaneId>(LOCAL_PLANE);
  state = $state<AuditState>({ kind: "idle" });
  /** "Show identifiers": off by default, and again on every Plane switch. */
  reveal = $state(false);
  exporting = $state<ExportState>({ kind: "idle" });
  epoch = $state<EpochDialog>({ kind: "closed" });

  private hooks: AuditHooks;
  private loaded = false;
  private disposed = false;
  /** Bumped on a Plane switch, a new Jet service start and a reveal change. */
  private generation = 0;
  private request = 0;
  private dialogRequest = 0;
  /** Uncertain epoch sends, kept per Plane across dialog closes. */
  private retained = new Map<PlaneId, { review: EpochReview; error: PublicError }>();

  constructor(hooks: AuditHooks) {
    this.hooks = hooks;
  }

  select(planeId: PlaneId): void {
    this.planeId = planeId;
    this.generation++;
    this.dialogRequest++;
    this.loaded = false;
    this.reveal = false;
    this.state = { kind: "idle" };
    this.exporting = { kind: "idle" };
    this.epoch = { kind: "closed" };
  }

  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.dialogRequest++;
  }

  /** Loads the first page once per Plane while the Audit section is shown. */
  async ensureLoaded(): Promise<void> {
    if (this.loaded) return;
    this.loaded = true;
    await this.load(null);
  }

  /** Reloads from the first page once loaded (a new audit period, the watcher resumed). */
  async reloadIfLoaded(): Promise<void> {
    if (!this.loaded) return;
    await this.refresh();
  }

  /** Jet on the Plane started again, possibly from an older store: nothing shown is kept. */
  async restarted(): Promise<void> {
    this.generation++;
    this.state = { kind: "idle" };
    if (this.epoch.kind === "review" || this.epoch.kind === "preparing") {
      this.dialogRequest++;
      this.epoch = { kind: "restarted" };
    }
    if (this.loaded) await this.load(null);
  }

  /** Reads the audit again from its oldest record. */
  async refresh(): Promise<void> {
    this.generation++;
    await this.load(null);
  }

  /** Shows or hides identifiers; the records are read again from the first page. */
  async setReveal(reveal: boolean): Promise<void> {
    if (reveal === this.reveal) return;
    this.reveal = reveal;
    await this.refresh();
  }

  /** "Load newer records". */
  async loadNewer(): Promise<void> {
    const state = this.state;
    if (state.kind !== "ready" || state.complete || state.entries.length >= MAX_SHOWN_RECORDS) return;
    await this.load(state.next);
  }

  /** Whether "Load newer records" can add anything. */
  get canLoadNewer(): boolean {
    return this.state.kind === "ready" && !this.state.complete && this.state.entries.length < MAX_SHOWN_RECORDS;
  }

  private shown(): AuditEntry[] {
    switch (this.state.kind) {
      case "loading":
      case "ready":
      case "offline":
      case "failed":
        return this.state.entries;
      default:
        return [];
    }
  }

  private async load(after: string | null, restartedOnce = false): Promise<void> {
    const { planeId, reveal } = this;
    const generation = this.generation;
    const request = ++this.request;
    // A reload from the first page replaces what is shown, but a failed
    // one keeps the last records that were read.
    const previous = this.shown();
    const kept = after === null ? [] : previous;
    this.state = { kind: "loading", entries: kept };
    try {
      const page = await loadSecurityAudit(planeId, after, reveal);
      if (this.disposed || generation !== this.generation || request !== this.request) return;
      const entries = [...kept, ...page.entries];
      const last = entries.at(-1);
      this.state = {
        kind: "ready",
        entries,
        next: last ? last.sequence : after,
        complete: page.complete,
      };
    } catch (thrown: unknown) {
      if (this.disposed || generation !== this.generation || request !== this.request) return;
      const error = publicError(thrown);
      if (error.restart?.reason === "pagination_stale" && !restartedOnce) {
        // The audit moved underneath the pages: start again from the oldest.
        await this.load(null, true);
        return;
      }
      this.hooks.observe(error);
      if (error.category === "unauthorized") this.state = { kind: "denied", error };
      else if (error.category === "incompatible" || error.protocolLimit !== null) {
        this.state = { kind: "unsupported", error };
      } else if (error.category === "offline" || (error.category === "unavailable" && error.retryable)) {
        this.state = { kind: "offline", entries: previous, error };
      } else this.state = { kind: "failed", entries: previous, error };
    }
  }

  // Evidence export ---------------------------------------------------------

  /** "Save audit evidence…": the shell asks where, then writes the file. */
  async exportEvidence(): Promise<void> {
    if (this.exporting.kind === "saving") return;
    const { planeId, generation } = this;
    this.exporting = { kind: "saving" };
    try {
      const result = await exportSecurityAudit(planeId);
      if (this.disposed || generation !== this.generation) return;
      if (result.kind === "canceled") {
        this.exporting = { kind: "idle" };
        return;
      }
      this.exporting = { kind: "saved", records: result.records, fileName: result.fileName };
      this.hooks.onExported();
    } catch (thrown: unknown) {
      if (this.disposed || generation !== this.generation) return;
      const error = publicError(thrown);
      this.hooks.observe(error);
      this.exporting = { kind: "failed", error };
    }
  }

  // New audit period --------------------------------------------------------

  /** Opens the review, or the uncertain request still waiting to be confirmed. */
  async prepareEpoch(): Promise<void> {
    if (this.epoch.kind === "preparing" || this.epoch.kind === "sending") return;
    const { planeId } = this;
    const request = ++this.dialogRequest;
    const retained = this.retained.get(planeId);
    if (retained) {
      this.epoch = { kind: "retry_epoch", review: retained.review, error: retained.error };
      return;
    }
    this.epoch = { kind: "preparing" };
    try {
      const review = await prepareRecoveryAction(planeId, { kind: "begin_audit_epoch" });
      if (!this.current(request, planeId)) return;
      if (review.kind !== "begin_audit_epoch") {
        // The shell reviewed something else: never offer it here.
        this.epoch = { kind: "refused", error: publicError(new Error("unexpected review")) };
        return;
      }
      this.epoch = { kind: "review", review };
    } catch (thrown: unknown) {
      if (!this.current(request, planeId)) return;
      const error = publicError(thrown);
      this.hooks.observe(error);
      this.epoch = isStaleEpochError(error) ? { kind: "stale", error } : { kind: "refused", error };
    }
  }

  /** Sends the reviewed new audit period, or resends an uncertain one unchanged. */
  async confirmEpoch(): Promise<void> {
    if (this.epoch.kind !== "review" && this.epoch.kind !== "retry_epoch") return;
    const { review } = this.epoch;
    const { planeId } = this;
    const request = this.dialogRequest;
    this.epoch = { kind: "sending", review };
    try {
      const outcome = await executeRecoveryAction(planeId, review.reviewId);
      this.retained.delete(planeId);
      if (!this.current(request, planeId)) {
        if (outcome.kind === "epoch_begun") this.hooks.securityChanged();
        return;
      }
      switch (outcome.kind) {
        case "epoch_begun":
          this.epoch = { kind: "done", epoch: outcome.epoch, planeLabel: review.planeLabel };
          this.exporting = { kind: "idle" };
          this.hooks.securityChanged();
          return;
        case "refused":
        case "unconfirmed":
          this.hooks.observe(outcome.error);
          this.epoch = isStaleEpochError(outcome.error)
            ? { kind: "stale", error: outcome.error }
            : { kind: "refused", error: outcome.error };
          return;
        default:
          this.epoch = { kind: "refused", error: publicError(new Error("unexpected outcome")) };
      }
    } catch (thrown: unknown) {
      // Uncertain: the shell kept the review attempted; the same review
      // resends the same Command ID.
      const error = publicError(thrown);
      this.hooks.observe(error);
      this.retained.set(planeId, { review, error });
      if (!this.current(request, planeId)) return;
      this.epoch = { kind: "retry_epoch", review, error };
    }
  }

  /** Closes the dialog; a request being sent stays until its outcome. */
  closeEpoch(): void {
    if (this.epoch.kind === "sending") return;
    this.dialogRequest++;
    this.epoch = { kind: "closed" };
  }

  /** Reload from a stale dialog: close it and read the audit again. */
  async reloadEpoch(): Promise<void> {
    this.closeEpoch();
    this.hooks.securityChanged();
  }

  private current(request: number, planeId: PlaneId): boolean {
    return !this.disposed && request === this.dialogRequest && planeId === this.planeId;
  }
}
