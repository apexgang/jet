import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { localServiceMark, mark } from "$lib/features/shell/timing";
import {
  executeLocalServiceRollback,
  isCurrentView,
  isProvisioning,
  prepareLocalServiceRollback,
  repairLocalService,
  watchLocalService,
  type LocalServiceRollbackReview,
  type LocalServiceView,
} from "$lib/jet/local-service";

/** Settings › Versions' reviewed rollback, one step at a time. */
export type RollbackDialog =
  | { kind: "closed" }
  | { kind: "preparing" }
  | { kind: "review"; review: LocalServiceRollbackReview }
  | { kind: "sending"; review: LocalServiceRollbackReview }
  | { kind: "done"; view: LocalServiceView }
  /** Nothing changed: the review was refused, expired or went stale. */
  | { kind: "refused"; error: PublicError };

/**
 * One window's view of the local Jet service (Wave 4 §A). It owns the
 * window's native watcher: `start` registers it once, the shell stops it
 * when the window closes, and `dispose` drops every later completion.
 */
export class LocalServiceSession {
  view = $state<LocalServiceView | null>(null);
  /** The watcher could not start; `view` may be missing or old. */
  error = $state<PublicError | null>(null);
  repairing = $state(false);
  /** Why the last Repair request itself failed (not the service). */
  repairError = $state<PublicError | null>(null);
  rollback = $state<RollbackDialog>({ kind: "closed" });

  private started = false;
  private disposed = false;
  /** Bumped when the rollback dialog closes. */
  private dialogRequest = 0;
  private onChange: (view: LocalServiceView, previous: LocalServiceView | null) => void;

  constructor(onChange: (view: LocalServiceView, previous: LocalServiceView | null) => void = () => undefined) {
    this.onChange = onChange;
  }

  /** The shell is installing, updating, starting or checking the service. */
  get provisioning(): boolean {
    return this.view !== null && isProvisioning(this.view.phase);
  }

  /** Registers this window's watcher; later calls do nothing. */
  async start(): Promise<void> {
    if (this.started) return;
    this.started = true;
    try {
      const initial = await watchLocalService((view) => {
        if (this.disposed || !isView(view)) return;
        this.apply(view);
      });
      // A view pushed before this answer arrived has a higher revision.
      if (!this.disposed && isView(initial)) this.apply(initial);
    } catch (error: unknown) {
      if (!this.disposed) this.error = publicError(error);
    }
  }

  dispose(): void {
    this.disposed = true;
    this.dialogRequest++;
  }

  /** Runs the native decision table again: install, update or start. */
  async repair(): Promise<void> {
    if (this.repairing || this.provisioning) return;
    this.repairing = true;
    this.repairError = null;
    try {
      const view = await repairLocalService();
      if (!this.disposed && isView(view)) this.apply(view);
    } catch (error: unknown) {
      if (!this.disposed) this.repairError = publicError(error);
    } finally {
      this.repairing = false;
    }
  }

  async prepareRollback(): Promise<void> {
    if (this.rollback.kind !== "closed") return;
    const request = ++this.dialogRequest;
    this.rollback = { kind: "preparing" };
    try {
      const review = await prepareLocalServiceRollback();
      if (this.disposed || request !== this.dialogRequest) return;
      this.rollback = { kind: "review", review };
    } catch (error: unknown) {
      if (this.disposed || request !== this.dialogRequest) return;
      this.rollback = { kind: "refused", error: publicError(error) };
    }
  }

  /** Sends the reviewed rollback once. */
  async confirmRollback(): Promise<void> {
    if (this.rollback.kind !== "review") return;
    const review = this.rollback.review;
    const request = this.dialogRequest;
    this.rollback = { kind: "sending", review };
    try {
      const view = await executeLocalServiceRollback(review.reviewId);
      if (this.disposed || !isView(view)) return;
      this.apply(view);
      if (request === this.dialogRequest) this.rollback = { kind: "done", view };
    } catch (error: unknown) {
      if (this.disposed || request !== this.dialogRequest) return;
      this.rollback = { kind: "refused", error: publicError(error) };
    }
  }

  /** Closes the dialog. A rollback already sent still finishes natively. */
  closeRollback(): void {
    if (this.rollback.kind === "sending") return;
    this.dialogRequest++;
    this.rollback = { kind: "closed" };
  }

  /** Shows `view` unless a newer one is shown already (a reply can cross a push). */
  private apply(view: LocalServiceView): void {
    const previous = this.view;
    if (!isCurrentView(view, previous)) return;
    // The release journey reads which provisioning phases this window saw.
    if (previous?.phase !== view.phase) mark(localServiceMark(view.phase));
    this.view = view;
    this.error = null;
    this.onChange(view, previous);
  }
}

/** A shell answer this session can show (a missing one is ignored). */
function isView(value: unknown): value is LocalServiceView {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { phase?: unknown }).phase === "string" &&
    typeof (value as { revision?: unknown }).revision === "number"
  );
}
