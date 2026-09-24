import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import type { PlaneId } from "$lib/jet/planes";
import {
  loadTrash,
  loadTrashStatus,
  previewTrash,
  resolveConversationNames,
  restoreConversation,
  trashConversation,
  type TrashEntry,
  type TrashMode,
  type TrashPreview,
  type TrashView,
} from "$lib/jet/retention";

import {
  namesToResolve,
  restoreRefusalReloads,
  sectionFromError,
  sectionFromView,
  sectionView,
  type TrashSectionState,
} from "./model";

/** What the main window tells Jet Trash about Planes and tasks. */
export type TrashHost = {
  /** Registered Planes, in registry order. */
  planeIds(): PlaneId[];
  /** A title from the task list already loaded for that Plane, or null. */
  knownTitle(planeId: PlaneId, conversationId: string): string | null;
  /** The outcome of a request on a Plane: a refusal, or null when admitted. */
  observe(planeId: PlaneId, error: PublicError | null): void;
};

/** Restore on one row (or the task banner), keyed by Plane and task. */
export type RowAction =
  | { kind: "idle" }
  | { kind: "restoring" }
  | { kind: "restored" }
  | { kind: "refused"; error: PublicError }
  | { kind: "uncertain"; error: PublicError };

/** The selected task's own Trash state, read from its retention preview. */
export type Banner =
  | { kind: "none" }
  | { kind: "loading" }
  | { kind: "trashed"; entry: TrashEntry }
  | { kind: "unknown"; error: PublicError };

type Reviewing = { planeId: PlaneId; preview: TrashPreview; mode: TrashMode; stopAcknowledged: boolean };

/** The Move to Trash dialog (wave 3.3 §7.2). Forget is selected when it opens. */
export type MoveDialogState =
  | { kind: "closed" }
  | { kind: "loading"; planeId: PlaneId; conversationId: string }
  | ({ kind: "review" } & Reviewing)
  | ({ kind: "review_unchecked" } & Reviewing)
  | ({ kind: "sending" } & Reviewing)
  | ({ kind: "uncertain"; error: PublicError } & Reviewing)
  | { kind: "already"; planeId: PlaneId; preview: TrashPreview }
  | { kind: "stale"; planeId: PlaneId; preview: TrashPreview; error: PublicError }
  | { kind: "refused"; planeId: PlaneId; preview: TrashPreview; error: PublicError }
  | { kind: "trashed"; planeId: PlaneId; preview: TrashPreview; entry: TrashEntry }
  | { kind: "failed"; planeId: PlaneId; conversationId: string; error: PublicError };

type Uncertain = Extract<MoveDialogState, { kind: "uncertain" }>;

/**
 * Refusals of the native attempt check: the review is still pending, so an
 * uncertain request stays retained for "Try again".
 */
const CHECK_REFUSALS = new Set([
  "retention.mode_locked",
  "retention.stop_unacknowledged",
  "retention.request_unresolved",
  "client.review_plane_mismatch",
]);

/**
 * Jet Trash in the main window: one section per Plane, task names, Restore,
 * the selected task's banner and the Move to Trash dialog. Every Plane keeps
 * its own state and generation, so one failing Plane never blanks another.
 */
export class TrashSession {
  sections = $state<Record<PlaneId, TrashSectionState>>({});
  /** Names from the native lookup, keyed by Plane and task. Null: unavailable. */
  names = $state<Record<string, string | null>>({});
  rows = $state<Record<string, RowAction>>({});
  banner = $state<{ planeId: PlaneId; conversationId: string; state: Banner } | null>(null);
  dialog = $state<MoveDialogState>({ kind: "closed" });
  /** The Jet Trash destination is shown: stale sections reload at once. */
  visible = $state(false);

  private generations = new Map<PlaneId, number>();
  private loads = new Map<PlaneId, number>();
  private bannerRequest = 0;
  private dialogRequest = 0;
  /** Uncertain requests survive closing the dialog, per Plane and task. */
  private retained = new Map<string, Uncertain>();
  private readonly host: TrashHost;
  private readonly now: () => number;

  constructor(host: TrashHost, now: () => number = Date.now) {
    this.host = host;
    this.now = now;
  }

  section(planeId: PlaneId): TrashSectionState {
    return this.sections[planeId] ?? { kind: "loading" };
  }

  rowAction(planeId: PlaneId, conversationId: string): RowAction {
    return this.rows[scopeKey(planeId, conversationId)] ?? { kind: "idle" };
  }

  /** A task's title: the loaded task list first, then the native lookup. */
  title(planeId: PlaneId, conversationId: string): string | null {
    return this.host.knownTitle(planeId, conversationId) ?? this.names[scopeKey(planeId, conversationId)] ?? null;
  }

  /** The banner state for this task, or none when it belongs to another. */
  bannerFor(planeId: PlaneId, conversationId: string | null): Banner {
    const banner = this.banner;
    return banner && banner.planeId === planeId && banner.conversationId === conversationId
      ? banner.state
      : { kind: "none" };
  }

  /** The destination opened: every registered Plane's section is read. */
  show(): void {
    this.visible = true;
    const registered = new Set(this.host.planeIds());
    for (const planeId of Object.keys(this.sections)) {
      if (!registered.has(planeId)) this.forget(planeId);
    }
    for (const planeId of registered) void this.load(planeId);
  }

  hide(): void {
    this.visible = false;
  }

  /** Reads one Plane's Jet Trash. Only that Plane's section changes. */
  async load(planeId: PlaneId): Promise<void> {
    const generation = this.generation(planeId);
    const request = (this.loads.get(planeId) ?? 0) + 1;
    this.loads.set(planeId, request);
    const current = () => generation === this.generation(planeId) && request === this.loads.get(planeId);
    if (!sectionView(this.sections[planeId])) this.sections[planeId] = { kind: "loading" };
    let view: TrashView;
    try {
      view = await loadTrash(planeId);
    } catch (error: unknown) {
      if (!current()) return;
      this.sections[planeId] = sectionFromError(publicError(error), sectionView(this.sections[planeId]));
      return;
    }
    if (!current()) return;
    this.sections[planeId] = sectionFromView(view, this.now());
    await this.resolveNames(planeId, view, generation);
  }

  /** Something in the Plane's Jet Trash changed: reload now, or when shown. */
  markStale(planeId: PlaneId): void {
    const state = this.sections[planeId];
    if (state?.kind === "ready" || state?.kind === "empty") {
      this.sections[planeId] = { ...state, freshness: "stale" };
    }
    if (this.visible && state) void this.load(planeId);
  }

  /** A setting changed: the grace days shown may be out of date. */
  markGraceStale(planeId: PlaneId): void {
    this.markStale(planeId);
  }

  /**
   * The Plane's daemon started again: its store may be older than what is
   * shown, so its section, names and banner are read again from scratch.
   */
  reset(planeId: PlaneId): void {
    this.drop(planeId);
    if (this.visible) void this.load(planeId);
    this.refreshBanner(planeId);
  }

  /** The Plane's feed dropped: keep the last list, marked offline. */
  offline(planeId: PlaneId): void {
    this.bump(planeId);
    const state = this.sections[planeId];
    if (state && state.kind !== "denied" && state.kind !== "unsupported") {
      this.sections[planeId] = { kind: "offline", last: sectionView(state) };
    }
  }

  /** The Plane's feed is back: an unavailable section is read again. */
  reconnected(planeId: PlaneId): void {
    const state = this.sections[planeId];
    if (!state) return;
    if (this.visible || state.kind === "offline" || state.kind === "failed") void this.load(planeId);
  }

  /** The Plane was forgotten on this computer. */
  forget(planeId: PlaneId): void {
    this.drop(planeId);
    if (this.banner?.planeId === planeId) this.clearBanner();
  }

  /** Restore from a row or the banner. Rows leave the list only on reload. */
  async restore(planeId: PlaneId, conversationId: string): Promise<void> {
    const key = scopeKey(planeId, conversationId);
    if (this.rows[key]?.kind === "restoring") return;
    this.rows[key] = { kind: "restoring" };
    try {
      const outcome = await restoreConversation(planeId, conversationId);
      if (outcome.kind === "restored") {
        this.host.observe(planeId, null);
        this.rows[key] = { kind: "restored" };
        await this.changed(planeId, conversationId);
        // The reload dropped the row; a later staging starts idle.
        if (this.rows[key]?.kind === "restored") delete this.rows[key];
        return;
      }
      this.host.observe(planeId, outcome.error);
      this.rows[key] = { kind: "refused", error: outcome.error };
      if (restoreRefusalReloads(outcome.error)) await this.changed(planeId, conversationId);
    } catch (error: unknown) {
      const failure = publicError(error);
      this.host.observe(planeId, failure);
      // The same staging keeps its native Command ID: Try again resends it.
      this.rows[key] = { kind: "uncertain", error: failure };
    }
  }

  /** Reads the selected task's Trash state. */
  async loadBanner(planeId: PlaneId, conversationId: string): Promise<void> {
    const request = ++this.bannerRequest;
    if (this.banner?.planeId !== planeId || this.banner.conversationId !== conversationId) {
      this.banner = { planeId, conversationId, state: { kind: "loading" } };
    }
    let state: Banner;
    try {
      const status = await loadTrashStatus(planeId, conversationId);
      state = status.trash ? { kind: "trashed", entry: status.trash } : { kind: "none" };
    } catch (error: unknown) {
      state = { kind: "unknown", error: publicError(error) };
    }
    if (request !== this.bannerRequest) return;
    this.banner = { planeId, conversationId, state };
  }

  /** Reads the banner again when it shows a task on `planeId`. */
  refreshBanner(planeId: PlaneId, conversationId: string | null = null): void {
    const banner = this.banner;
    if (!banner || banner.planeId !== planeId) return;
    if (conversationId !== null && banner.conversationId !== conversationId) return;
    void this.loadBanner(banner.planeId, banner.conversationId);
  }

  clearBanner(): void {
    this.bannerRequest += 1;
    this.banner = null;
  }

  /**
   * Opens Move to Trash for a task: a retained uncertain request comes back
   * with its locked mode; otherwise a new native review is read.
   */
  async openMove(planeId: PlaneId, conversationId: string): Promise<void> {
    const request = ++this.dialogRequest;
    const retained = this.retained.get(scopeKey(planeId, conversationId));
    if (retained) {
      this.dialog = retained;
      return;
    }
    this.dialog = { kind: "loading", planeId, conversationId };
    let next: MoveDialogState;
    try {
      next = reviewState(planeId, await previewTrash(planeId, conversationId));
    } catch (error: unknown) {
      const failure = publicError(error);
      this.host.observe(planeId, failure);
      next = { kind: "failed", planeId, conversationId, error: failure };
    }
    // A late preview for an earlier or closed dialog is dropped.
    if (request !== this.dialogRequest) return;
    this.dialog = next;
  }

  chooseMode(mode: TrashMode): void {
    const dialog = this.dialog;
    if (dialog.kind === "review" || dialog.kind === "review_unchecked") this.dialog = { ...dialog, mode };
  }

  acknowledgeStop(acknowledged: boolean): void {
    const dialog = this.dialog;
    if (dialog.kind === "review" || dialog.kind === "review_unchecked") {
      this.dialog = { ...dialog, stopAcknowledged: acknowledged };
    }
  }

  /** Delete everywhere stays blocked until the stop is acknowledged. */
  get canConfirm(): boolean {
    const dialog = this.dialog;
    if (dialog.kind === "uncertain") return true;
    if (dialog.kind !== "review" && dialog.kind !== "review_unchecked") return false;
    return dialog.mode === "forget" || !dialog.preview.stopAcknowledgementRequired || dialog.stopAcknowledged;
  }

  /** Sends the reviewed request, or resends an uncertain one unchanged. */
  async confirm(): Promise<void> {
    const dialog = this.dialog;
    if ((dialog.kind !== "review" && dialog.kind !== "review_unchecked" && dialog.kind !== "uncertain") || !this.canConfirm) {
      return;
    }
    const { planeId, preview, mode, stopAcknowledged } = dialog;
    const conversationId = preview.conversationId;
    const key = scopeKey(planeId, conversationId);
    const request = ++this.dialogRequest;
    this.dialog = { kind: "sending", planeId, preview, mode, stopAcknowledged };
    let next: MoveDialogState;
    try {
      const outcome = await trashConversation(
        planeId,
        preview.reviewId,
        mode,
        mode === "delete_everywhere" && stopAcknowledged,
      );
      if (outcome.kind === "trashed") {
        this.retained.delete(key);
        this.host.observe(planeId, null);
        next = { kind: "trashed", planeId, preview, entry: outcome.entry };
        void this.changed(planeId, conversationId);
      } else {
        const { error } = outcome;
        this.host.observe(planeId, error);
        if (dialog.kind === "uncertain" && CHECK_REFUSALS.has(error.code)) {
          next = { ...dialog, error };
          this.retained.set(key, next);
        } else {
          this.retained.delete(key);
          next =
            error.code === "retention.review_stale"
              ? { kind: "stale", planeId, preview, error }
              : { kind: "refused", planeId, preview, error };
        }
      }
    } catch (error: unknown) {
      const failure = publicError(error);
      this.host.observe(planeId, failure);
      const uncertain: Uncertain = { kind: "uncertain", planeId, preview, mode, stopAcknowledged, error: failure };
      this.retained.set(key, uncertain);
      next = uncertain;
    }
    if (request === this.dialogRequest) this.dialog = next;
  }

  /** "Review again" after a stale review or a failed preview. */
  async reviewAgain(): Promise<void> {
    const dialog = this.dialog;
    if (dialog.kind === "stale" || dialog.kind === "refused") {
      await this.openMove(dialog.planeId, dialog.preview.conversationId);
    } else if (dialog.kind === "failed") {
      await this.openMove(dialog.planeId, dialog.conversationId);
    }
  }

  /** Closes the dialog unless a request is being sent. Returns whether it closed. */
  closeMove(): boolean {
    if (this.dialog.kind === "sending") return false;
    this.dialogRequest += 1;
    this.dialog = { kind: "closed" };
    return true;
  }

  /** Restore from the dialog when the task is already in Jet Trash. */
  async restoreFromDialog(): Promise<void> {
    const dialog = this.dialog;
    if (dialog.kind !== "already") return;
    this.closeMove();
    await this.restore(dialog.planeId, dialog.preview.conversationId);
  }

  /** After a Command changed a task's Trash state: read what shows it. */
  private async changed(planeId: PlaneId, conversationId: string): Promise<void> {
    this.refreshBanner(planeId, conversationId);
    if (this.sections[planeId] || this.visible) await this.load(planeId);
  }

  private async resolveNames(planeId: PlaneId, view: TrashView, generation: number): Promise<void> {
    const known = (id: string) =>
      this.host.knownTitle(planeId, id) !== null || scopeKey(planeId, id) in this.names;
    for (const chunk of namesToResolve(view.entries, known)) {
      let titles: Array<[string, string | null]>;
      try {
        const names = await resolveConversationNames(planeId, chunk);
        titles = chunk.map((id) => [id, names.find((name) => name.conversationId === id)?.title ?? null]);
      } catch {
        titles = chunk.map((id) => [id, null]);
      }
      if (generation !== this.generation(planeId)) return;
      for (const [id, title] of titles) this.names[scopeKey(planeId, id)] = title;
    }
  }

  private drop(planeId: PlaneId): void {
    this.bump(planeId);
    delete this.sections[planeId];
    const prefix = scopeKey(planeId, "");
    for (const key of Object.keys(this.names)) if (key.startsWith(prefix)) delete this.names[key];
    for (const key of Object.keys(this.rows)) if (key.startsWith(prefix)) delete this.rows[key];
  }

  private generation(planeId: PlaneId): number {
    return this.generations.get(planeId) ?? 0;
  }

  private bump(planeId: PlaneId): void {
    this.generations.set(planeId, this.generation(planeId) + 1);
  }
}

function reviewState(planeId: PlaneId, preview: TrashPreview): MoveDialogState {
  if (preview.trash) return { kind: "already", planeId, preview };
  const review = { planeId, preview, mode: "forget" as const, stopAcknowledged: false };
  return preview.workspaceUnchecked ? { kind: "review_unchecked", ...review } : { kind: "review", ...review };
}

function scopeKey(planeId: PlaneId, conversationId: string): string {
  return `${planeId}\u0000${conversationId}`;
}
