import {
  closePlaneFeed,
  openPlaneFeed,
  type ConnectionSnapshot,
  type PlaneUpdate,
  type PublicError,
} from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import {
  LOCAL_PLANE,
  forgetRemotePlane,
  listPlanes,
  loadPlaneDetail,
  planeLabel,
  type Plane,
  type PlaneDetail,
  type PlaneId,
  type PlanesSnapshot,
} from "$lib/jet/planes";

import { PlaneEnrollment } from "./enrollment.svelte";
import { aggregateStatus, launchNotices, planeAttentionCount } from "./model";
import { OwnerPairing } from "./pairing.svelte";

/** Deep-link targets inside the Planes destination. */
export type PlanesFocus = "add" | "repair" | "pairing" | "clients" | "detail";

export type PlaneDetailState =
  | { kind: "loading"; planeId: PlaneId }
  | { kind: "ready"; planeId: PlaneId; detail: PlaneDetail }
  | { kind: "failed"; planeId: PlaneId; error: PublicError };

/** Who handles one Plane's feed callbacks. `DesktopSession` implements it. */
export type FeedHandler = {
  receive(planeId: PlaneId, update: PlaneUpdate): void;
  opened(planeId: PlaneId, snapshot: ConnectionSnapshot): void;
  openFailed(planeId: PlaneId, error: PublicError): void;
  /** A Plane was paired (new) or paired again (`repaired`). */
  planePaired?(planeId: PlaneId, repaired: boolean): Promise<void> | void;
  /** A Plane was forgotten on this computer. */
  planeForgotten?(planeId: PlaneId): void;
};

export type ForgetState =
  | { kind: "idle" }
  | { kind: "confirming"; planeId: PlaneId }
  | { kind: "forgetting"; planeId: PlaneId }
  | { kind: "failed"; planeId: PlaneId; error: PublicError };

/**
 * The webview's mirror of the native Plane registry plus one live feed per
 * Plane. The registry snapshot is the only source of Plane labels, health and
 * connection state; it is re-read after every feed transition and never
 * connects to a Plane by itself.
 */
export class PlanesSession {
  snapshot = $state<PlanesSnapshot | null>(null);
  error = $state<PublicError | null>(null);
  selectedPlaneId = $state<PlaneId>(LOCAL_PLANE);
  detail = $state<PlaneDetailState | null>(null);
  /** Bumped on every deep link so the panel can move focus even to the same section. */
  focusRequest = $state<{ section: PlanesFocus; serial: number } | null>(null);
  noticeDismissed = $state(false);
  /**
   * A Plane switch held back because a one-time code is on screen. The code
   * stops working when the switch is confirmed (Stop closes the gate).
   */
  pendingSwitch = $state<{ planeId: PlaneId; focus: PlanesFocus | null } | null>(null);
  /** Owner-side Pairing for the selected Plane. */
  readonly pairing: OwnerPairing;
  /** Add a Plane / Pair again. */
  readonly enrollment: PlaneEnrollment;
  forgetState = $state<ForgetState>({ kind: "idle" });

  private refreshRequest = 0;
  private detailRequest = 0;
  private focusSerial = 0;
  /** One feed per Plane: callbacks from a replaced feed of a Plane are ignored. */
  private feeds = new Map<PlaneId, { generation: number; feedId: string | null }>();
  private feedGeneration = 0;

  private readonly handler: FeedHandler;

  constructor(handler: FeedHandler) {
    this.handler = handler;
    this.pairing = new OwnerPairing(
      (planeId) => this.label(planeId),
      (planeId) => this.forget(planeId),
    );
    this.enrollment = new PlaneEnrollment((plane, repaired) => this.paired(plane, repaired));
  }

  private async paired(plane: Plane, repaired: boolean): Promise<void> {
    await this.refresh();
    await this.handler.planePaired?.(plane.planeId, repaired);
    if (this.selectedPlaneId === plane.planeId) void this.loadDetail();
  }

  /** Opens the wizard at the SSH address. */
  startAdd(): void {
    this.enrollment.startAdd();
  }

  /** Pair again, in place, for a registered remote Plane. */
  startRepair(planeId: PlaneId): Promise<void> {
    const plane = this.plane(planeId);
    if (!plane || plane.kind !== "remote") return Promise.resolve();
    return this.enrollment.startRepair(planeId, plane.label);
  }

  /** Shows the inline Forget choice for a remote Plane. */
  askForget(planeId: PlaneId): void {
    if (this.plane(planeId)?.kind === "remote") this.forgetState = { kind: "confirming", planeId };
  }

  cancelForget(): void {
    if (this.forgetState.kind !== "forgetting") this.forgetState = { kind: "idle" };
  }

  /**
   * Revoke this computer's access on the Plane, then forget it. Runs the
   * reviewed revoke (ClientChangeDialog); on a refusal or an uncertain
   * outcome it stops and shows the error, and plain Forget stays available.
   */
  async revokeThenForget(planeId: PlaneId): Promise<void> {
    const view = this.pairing.planeId === planeId ? this.pairing.view : null;
    if (!view) return;
    this.forgetState = { kind: "idle" };
    await this.pairing.prepareChange(view.thisClientId, "revoke", { thenForget: true });
  }

  /**
   * Forget only: removes the Plane from this computer and hides its tasks.
   * Nothing is sent to the Plane, which still lists this computer as paired.
   */
  async forget(planeId: PlaneId): Promise<void> {
    if (planeId === LOCAL_PLANE) return;
    this.forgetState = { kind: "forgetting", planeId };
    try {
      const snapshot = await forgetRemotePlane(planeId);
      this.refreshRequest += 1;
      this.snapshot = snapshot;
      this.error = null;
      this.closeFeed(planeId);
      if (this.pairing.planeId === planeId) this.pairing.leave();
      if (this.selectedPlaneId === planeId) {
        this.selectedPlaneId = LOCAL_PLANE;
        this.detail = null;
      }
      this.forgetState = { kind: "idle" };
      this.handler.planeForgotten?.(planeId);
    } catch (error: unknown) {
      this.forgetState = { kind: "failed", planeId, error: publicError(error) };
    }
  }

  get planes(): Plane[] {
    return this.snapshot?.planes ?? [];
  }

  plane(planeId: PlaneId): Plane | null {
    return this.planes.find((plane) => plane.planeId === planeId) ?? null;
  }

  has(planeId: PlaneId): boolean {
    return this.plane(planeId) !== null;
  }

  label(planeId: PlaneId): string {
    return planeLabel(this.snapshot, planeId);
  }

  get multiple(): boolean {
    return this.planes.length > 1;
  }

  get attentionCount(): number {
    return planeAttentionCount(this.planes, this.pairing.awaiting);
  }

  get aggregate() {
    return this.planes.length > 0 ? aggregateStatus(this.planes) : null;
  }

  get selectedPlane(): Plane | null {
    return this.plane(this.selectedPlaneId);
  }

  /** What launch did to the saved Planes and this computer's identity. */
  get notices(): string[] {
    return this.snapshot && !this.noticeDismissed ? launchNotices(this.snapshot) : [];
  }

  /** Reads the native registry snapshot. It never connects to a Plane. */
  async refresh(): Promise<void> {
    const request = ++this.refreshRequest;
    try {
      const snapshot = await listPlanes();
      if (request !== this.refreshRequest) return;
      this.snapshot = snapshot;
      this.error = null;
      if (!snapshot.planes.some((plane) => plane.planeId === this.selectedPlaneId)) {
        this.selectedPlaneId = LOCAL_PLANE;
      }
    } catch (error: unknown) {
      if (request === this.refreshRequest) this.error = publicError(error);
    }
  }

  /** Selects a Plane in the Planes destination and reads its detail. */
  select(planeId: PlaneId, focus: PlanesFocus | null = null): void {
    const known = this.has(planeId) ? planeId : LOCAL_PLANE;
    if (known !== this.selectedPlaneId && this.pairing.holdsCode) {
      this.pendingSwitch = { planeId: known, focus };
      return;
    }
    this.pendingSwitch = null;
    const changed = known !== this.selectedPlaneId || this.detail?.planeId !== known;
    this.selectedPlaneId = known;
    if (focus) this.focusRequest = { section: focus, serial: ++this.focusSerial };
    if (changed || this.detail?.kind === "failed") void this.loadDetail();
  }

  /** Confirms a held switch: Stop pairing (closes the gate), then switch. */
  async confirmSwitch(): Promise<void> {
    const target = this.pendingSwitch;
    if (!target) return;
    await this.pairing.stop();
    if (this.pairing.holdsCode) return;
    this.pendingSwitch = null;
    this.select(target.planeId, target.focus);
  }

  cancelSwitch(): void {
    this.pendingSwitch = null;
  }

  /** Status, capabilities and knowledge of the selected Plane. */
  async loadDetail(): Promise<void> {
    const planeId = this.selectedPlaneId;
    const request = ++this.detailRequest;
    if (this.detail?.planeId !== planeId || this.detail.kind === "failed") {
      this.detail = { kind: "loading", planeId };
    }
    try {
      const detail = await loadPlaneDetail(planeId);
      if (request !== this.detailRequest || planeId !== this.selectedPlaneId) return;
      this.detail = { kind: "ready", planeId, detail };
    } catch (error: unknown) {
      if (request !== this.detailRequest || planeId !== this.selectedPlaneId) return;
      this.detail = { kind: "failed", planeId, error: publicError(error) };
    }
    // The detail read seeds knowledge and health natively; mirror them.
    await this.refresh();
  }

  /**
   * The Plane's daemon started again: a detail shown for it names the old
   * start's versions and capabilities, so it is read again.
   */
  planeRestarted(planeId: PlaneId): void {
    if (planeId === this.selectedPlaneId && this.detail?.planeId === planeId) void this.loadDetail();
  }

  hasFeed(planeId: PlaneId): boolean {
    return this.feeds.has(planeId);
  }

  /** Opens a Plane's feed only if this session has never opened one. */
  async ensureFeed(planeId: PlaneId, after: string | null): Promise<void> {
    if (this.feeds.has(planeId)) return;
    await this.openFeed(planeId, after);
  }

  /**
   * (Re)opens one Plane's feed. Natively this replaces that Plane's previous
   * feed task; here, callbacks from the replaced feed are ignored. `reset` is
   * the user's Retry.
   */
  async openFeed(planeId: PlaneId, after: string | null, reset = false): Promise<void> {
    const generation = ++this.feedGeneration;
    this.feeds.set(planeId, { generation, feedId: null });
    const current = () => this.feeds.get(planeId)?.generation === generation;
    // The feed task runs before this call answers, so a drop and a redial's
    // status can arrive first. Its snapshot is then older than what they
    // said (perhaps an earlier daemon start) and is not applied over them.
    let superseded = false;
    try {
      const snapshot = await openPlaneFeed(
        (update) => {
          if (!current()) return;
          if (update.type === "connected" || update.type === "reconnecting" || update.type === "failed") {
            superseded = true;
          }
          this.handler.receive(planeId, update);
        },
        after,
        planeId,
        reset,
      );
      if (!current()) {
        void closePlaneFeed(snapshot.feedId).catch(() => undefined);
        return;
      }
      this.feeds.set(planeId, { generation, feedId: snapshot.feedId });
      if (!superseded) this.handler.opened(planeId, snapshot);
    } catch (error: unknown) {
      if (current()) this.handler.openFailed(planeId, publicError(error));
    }
    if (current()) void this.refresh();
  }

  /** Stops one Plane's feed natively; later callbacks from it are ignored. */
  closeFeed(planeId: PlaneId): void {
    const feed = this.feeds.get(planeId);
    this.feeds.delete(planeId);
    if (feed?.feedId) void closePlaneFeed(feed.feedId).catch(() => undefined);
  }

  /** Stops every Plane feed natively, for example when the window closes. */
  closeAll(): void {
    for (const [planeId, feed] of this.feeds) {
      this.feeds.set(planeId, { generation: ++this.feedGeneration, feedId: null });
      if (feed.feedId) void closePlaneFeed(feed.feedId).catch(() => undefined);
    }
  }
}
