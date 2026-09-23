import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import {
  confirmPairingRequest,
  executePairedClientChange,
  loadPairing,
  openPairingOffer,
  preparePairedClientChange,
  setPairingGate,
  type ClientChange,
  type ClientChangeReceipt,
  type ClientChangeReview,
  type PairingView,
} from "$lib/jet/pairing";
import type { PlaneId } from "$lib/jet/planes";

import {
  formatFingerprint,
  millisecondsLeft,
  normalizeAuthString,
  pairingPausedCopy,
  sectionData,
  sectionFromError,
  type Section,
} from "./model";

export type OwnerPairingState = Section<PairingView>;

export type OfferEnd = "expired" | "too_many_attempts" | "gate_closed" | "claimed";

/** The one-time code lives only in `shown`, and only while it can be used. */
export type OfferState =
  | { kind: "none" }
  | { kind: "opening" }
  | { kind: "shown"; offerId: string; code: string; expiresAtUnixMs: string; attemptsRemaining: number }
  | { kind: "already_disclosed"; offerId: string }
  | { kind: "stopping" }
  | { kind: "ended"; reason: OfferEnd };

export type ConfirmState =
  | { kind: "idle" }
  | { kind: "entering"; offerId: string; value: string }
  | { kind: "sending"; offerId: string }
  | { kind: "mismatch"; offerId: string }
  | { kind: "confirmed" };

export type ClientChangeState =
  | { kind: "none" }
  | { kind: "preparing"; clientId: string }
  | { kind: "review"; review: ClientChangeReview }
  | { kind: "sending"; review: ClientChangeReview }
  | { kind: "uncertain"; review: ClientChangeReview; error: PublicError }
  | { kind: "done"; receipt: ClientChangeReceipt; review: ClientChangeReview };

export type StopFailure = { error: PublicError; validUntilUnixMs: string | null };

/**
 * Owner-side Pairing for the Plane shown in the Planes destination. It never
 * holds the one-time code longer than it can be used: the code is dropped on
 * claim, on an ended refresh, when its expiry passes, on Stop and when the
 * section is left. Stop always closes the gate, because closing it is the
 * only way to end an offer.
 */
export class OwnerPairing {
  planeId = $state<PlaneId | null>(null);
  state = $state<OwnerPairingState>({ kind: "loading" });
  checkedAtUnixMs = $state<number | null>(null);
  offer = $state<OfferState>({ kind: "none" });
  confirm = $state<ConfirmState>({ kind: "idle" });
  change = $state<ClientChangeState>({ kind: "none" });
  actionError = $state<PublicError | null>(null);
  stopFailure = $state<StopFailure | null>(null);
  notice = $state<string | null>(null);
  busy = $state(false);
  /** Planes with a claim waiting for the owner's confirmation, for the badge. */
  awaiting = $state<Record<PlaneId, { awaitingConfirmation: boolean }>>({});

  /** Whether this flow opened the gate, so completion closes it again. */
  openedGateForThisPairing = false;

  private loadRequest = 0;
  private claimant: string | null = null;
  private knownClients = new Set<string>();
  /** Uncertain client changes survive switching Planes, per Plane. */
  private retained = new Map<PlaneId, Extract<ClientChangeState, { kind: "uncertain" }>>();
  /** Revoke-then-forget: forget this Plane once its revoke has applied. */
  private forgetAfter: PlaneId | null = null;
  private readonly labelOf: (planeId: PlaneId) => string;
  private readonly onForget: (planeId: PlaneId) => Promise<void>;

  constructor(
    labelOf: (planeId: PlaneId) => string,
    onForget: (planeId: PlaneId) => Promise<void> = async () => undefined,
  ) {
    this.labelOf = labelOf;
    this.onForget = onForget;
  }

  get view(): PairingView | null {
    return sectionData(this.state);
  }

  get label(): string {
    return this.planeId ? this.labelOf(this.planeId) : "this Plane";
  }

  /** Why mutations are unavailable right now, as user-facing copy. */
  get blockedReason(): string | null {
    const view = this.view;
    if (!view) return null;
    if (this.state.kind === "stale") {
      return `Showing information from an earlier check. Pairing changes need a current read of ${this.label}.`;
    }
    return pairingPausedCopy(view.mutations.allowed ? null : view.mutations.reason, this.label);
  }

  get canMutate(): boolean {
    return this.state.kind === "ready" && this.state.data.mutations.allowed && !this.busy;
  }

  /** Whether a usable one-time code is on screen. */
  get holdsCode(): boolean {
    return this.offer.kind === "shown";
  }

  /** Shows owner Pairing for a Plane. Switching Planes drops transient state. */
  show(planeId: PlaneId): void {
    if (planeId === this.planeId) return;
    this.reset();
    this.planeId = planeId;
    this.state = { kind: "loading" };
    this.checkedAtUnixMs = null;
    this.knownClients = new Set();
    const retained = this.retained.get(planeId);
    if (retained) this.change = retained;
    void this.load();
  }

  /**
   * Leaving the section: a shown code stops working. The gate is closed,
   * because that is the only way to end the offer on the Plane.
   */
  leave(): void {
    const planeId = this.planeId;
    const holding = this.offer.kind === "shown" || this.offer.kind === "already_disclosed";
    this.reset();
    this.planeId = null;
    if (holding && planeId) void setPairingGate(planeId, "closed").catch(() => undefined);
  }

  private reset(): void {
    this.loadRequest += 1;
    this.offer = { kind: "none" };
    this.confirm = { kind: "idle" };
    this.change = { kind: "none" };
    this.actionError = null;
    this.stopFailure = null;
    this.notice = null;
    this.busy = false;
    this.openedGateForThisPairing = false;
    this.claimant = null;
  }

  private current(planeId: PlaneId): boolean {
    return this.planeId === planeId;
  }

  /** Re-reads Pairing. A late reply for another Plane is discarded. */
  async load(): Promise<void> {
    const planeId = this.planeId;
    if (!planeId) return;
    const request = ++this.loadRequest;
    try {
      const view = await loadPairing(planeId);
      if (request !== this.loadRequest || !this.current(planeId)) return;
      this.apply(view);
    } catch (error: unknown) {
      if (request !== this.loadRequest || !this.current(planeId)) return;
      this.state = sectionFromError(publicError(error), this.view);
    }
  }

  /** A `pairing.*` event on a Plane: refresh what shows it. */
  pairingEvent(planeId: PlaneId): void {
    if (this.current(planeId)) {
      void this.load();
      return;
    }
    void loadPairing(planeId)
      .then((view) => this.track(view))
      .catch(() => undefined);
  }

  private track(view: PairingView): void {
    this.awaiting = {
      ...this.awaiting,
      [view.planeId]: { awaitingConfirmation: view.pending?.progress.kind === "awaiting_confirmation" },
    };
  }

  private apply(view: PairingView): void {
    this.loadRequest += 1;
    this.state = { kind: "ready", data: view };
    this.checkedAtUnixMs = Date.now();
    this.track(view);
    this.reconcile(view);
    this.knownClients = new Set(view.clients.map((client) => client.clientId));
  }

  /** Moves the local flow to what the Plane now reports. */
  private reconcile(view: PairingView): void {
    const pending = view.pending;
    const progress = pending?.progress ?? null;

    if (this.offer.kind === "shown" || this.offer.kind === "already_disclosed") {
      const offerId = this.offer.offerId;
      if (!pending || pending.offerId !== offerId) {
        this.offer = { kind: "ended", reason: view.gate === "closed" ? "gate_closed" : "expired" };
      } else if (progress?.kind === "ended") {
        this.offer = { kind: "ended", reason: progress.reason };
      } else if (progress?.kind !== "offered") {
        // Claimed: the code has done its job and is dropped.
        this.offer = { kind: "ended", reason: "claimed" };
      } else if (this.offer.kind === "shown") {
        this.offer = { ...this.offer, attemptsRemaining: pending.attemptsRemaining };
      }
    }

    if (progress?.kind === "awaiting_confirmation" && pending) {
      this.claimant = progress.clientId;
      const sameOffer =
        (this.confirm.kind === "entering" || this.confirm.kind === "sending" || this.confirm.kind === "mismatch") &&
        this.confirm.offerId === pending.offerId;
      if (!sameOffer) this.confirm = { kind: "entering", offerId: pending.offerId, value: "" };
    } else if (progress?.kind === "confirmed") {
      this.claimant = progress.clientId;
      this.confirm = { kind: "confirmed" };
    } else if (progress?.kind === "ended") {
      if (this.confirm.kind !== "idle") {
        this.confirm = { kind: "idle" };
        this.offer = { kind: "ended", reason: progress.reason };
      }
    }

    const claimant = this.claimant;
    const completed = claimant
      ? view.clients.find((client) => client.clientId === claimant && !this.knownClients.has(client.clientId))
      : undefined;
    if (completed) {
      this.claimant = null;
      this.confirm = { kind: "idle" };
      this.offer = { kind: "none" };
      this.notice = `${formatFingerprint(completed.fingerprint)} is now paired.`;
      if (this.openedGateForThisPairing && view.gate === "open") void this.closeAfterCompletion();
      else this.openedGateForThisPairing = false;
    }
  }

  private async closeAfterCompletion(): Promise<void> {
    const planeId = this.planeId;
    if (!planeId) return;
    this.openedGateForThisPairing = false;
    try {
      const view = await setPairingGate(planeId, "closed");
      if (!this.current(planeId)) return;
      this.notice = `${this.notice ?? ""} Pairing is closed again.`.trim();
      this.apply(view);
    } catch (error: unknown) {
      if (this.current(planeId)) this.actionError = publicError(error);
    }
  }

  /** "Pair a computer": opens the gate when closed, then opens an offer. */
  async pairComputer(): Promise<void> {
    const planeId = this.planeId;
    const view = this.view;
    if (!planeId || !view || !this.canMutate) return;
    this.busy = true;
    this.actionError = null;
    this.stopFailure = null;
    this.notice = null;
    this.offer = { kind: "opening" };
    try {
      if (view.gate === "closed") {
        const opened = await setPairingGate(planeId, "open");
        if (!this.current(planeId)) return;
        this.openedGateForThisPairing = true;
        this.apply(opened);
      }
      const disclosure = await openPairingOffer(planeId);
      if (!this.current(planeId)) return;
      this.offer = disclosure.code
        ? {
            kind: "shown",
            offerId: disclosure.offerId,
            code: disclosure.code,
            expiresAtUnixMs: disclosure.expiresAtUnixMs,
            attemptsRemaining: disclosure.attemptsRemaining,
          }
        : { kind: "already_disclosed", offerId: disclosure.offerId };
    } catch (error: unknown) {
      if (!this.current(planeId)) return;
      this.offer = { kind: "none" };
      this.actionError = publicError(error);
    } finally {
      if (this.current(planeId)) this.busy = false;
    }
    void this.load();
  }

  /** The code never reached this window; a new offer replaces the old one. */
  getNewCode(): Promise<void> {
    return this.pairComputer();
  }

  /**
   * Stop pairing, Reject and Close pairing: always closes the gate, even when
   * it was opened elsewhere. It stays available while trust changes are
   * paused, because it only reduces who may pair; jetd still decides.
   */
  async stop(): Promise<void> {
    const planeId = this.planeId;
    if (!planeId || this.offer.kind === "stopping") return;
    const previous = this.offer;
    const validUntilUnixMs =
      previous.kind === "shown" ? previous.expiresAtUnixMs : (this.view?.pending?.expiresAtUnixMs ?? null);
    this.offer = { kind: "stopping" };
    this.busy = true;
    this.actionError = null;
    this.stopFailure = null;
    try {
      const view = await setPairingGate(planeId, "closed");
      if (!this.current(planeId)) return;
      this.openedGateForThisPairing = false;
      this.claimant = null;
      this.confirm = { kind: "idle" };
      this.offer = { kind: "ended", reason: "gate_closed" };
      this.apply(view);
    } catch (error: unknown) {
      if (!this.current(planeId)) return;
      this.offer = previous;
      this.stopFailure = { error: publicError(error), validUntilUnixMs };
    } finally {
      if (this.current(planeId)) this.busy = false;
    }
  }

  /** Called on each countdown tick: a passed expiry drops the code. */
  expire(nowUnixMs: number): void {
    if (this.offer.kind === "shown" && millisecondsLeft(this.offer.expiresAtUnixMs, nowUnixMs) <= 0) {
      this.offer = { kind: "ended", reason: "expired" };
    }
  }

  setConfirmValue(value: string): void {
    if (this.confirm.kind !== "entering" && this.confirm.kind !== "mismatch") return;
    this.confirm = { kind: "entering", offerId: this.confirm.offerId, value: normalizeAuthString(value) };
  }

  /** Confirms a claim with the string typed from the other computer's screen. */
  async submitConfirm(): Promise<void> {
    const planeId = this.planeId;
    if (!planeId || this.confirm.kind !== "entering" || !this.canMutate) return;
    const { offerId, value } = this.confirm;
    this.busy = true;
    this.actionError = null;
    this.confirm = { kind: "sending", offerId };
    try {
      await confirmPairingRequest(planeId, offerId, value);
      if (!this.current(planeId)) return;
      this.confirm = { kind: "confirmed" };
    } catch (error: unknown) {
      if (!this.current(planeId)) return;
      const failure = publicError(error);
      if (failure.code === "pairing.authentication_string_mismatch") {
        this.confirm = { kind: "mismatch", offerId };
      } else {
        this.confirm = { kind: "entering", offerId, value };
        this.actionError = failure;
      }
    } finally {
      if (this.current(planeId)) this.busy = false;
    }
    void this.load();
  }

  /**
   * Prepares a client change natively. Enable is not destructive and runs at
   * once. With `thenForget`, an applied (or applied-unverified) revoke of
   * this computer also forgets the Plane here; a refusal or an uncertain
   * outcome stops before forgetting.
   */
  async prepareChange(
    clientId: string,
    change: ClientChange,
    options: { thenForget?: boolean } = {},
  ): Promise<void> {
    const planeId = this.planeId;
    if (!planeId || !this.canMutate || this.changeInFlight) return;
    this.forgetAfter = options.thenForget ? planeId : null;
    this.actionError = null;
    this.notice = null;
    this.change = { kind: "preparing", clientId };
    try {
      const review = await preparePairedClientChange(planeId, clientId, change);
      if (!this.current(planeId)) return;
      if (change === "enable") {
        await this.execute({ kind: "sending", review });
      } else {
        this.change = { kind: "review", review };
      }
    } catch (error: unknown) {
      if (!this.current(planeId)) return;
      this.change = { kind: "none" };
      this.actionError = publicError(error);
    }
  }

  get changeInFlight(): boolean {
    return this.change.kind === "preparing" || this.change.kind === "sending" || this.change.kind === "uncertain";
  }

  /** Sends the reviewed change, or resends the same review after uncertainty. */
  async executeChange(): Promise<void> {
    if (this.change.kind !== "review" && this.change.kind !== "uncertain") return;
    await this.execute({ kind: "sending", review: this.change.review });
  }

  cancelChange(): void {
    if (this.change.kind === "review" || this.change.kind === "done") {
      this.change = { kind: "none" };
      this.forgetAfter = null;
    }
  }

  /** "Forget {label}" after this computer lost access to a remote Plane. */
  async forgetPlane(): Promise<void> {
    if (this.change.kind !== "done") return;
    const { planeId } = this.change.review;
    this.change = { kind: "none" };
    this.forgetAfter = null;
    await this.onForget(planeId);
  }

  private async execute(sending: Extract<ClientChangeState, { kind: "sending" }>): Promise<void> {
    const { review } = sending;
    this.change = sending;
    try {
      const receipt = await executePairedClientChange(review.reviewId);
      this.retained.delete(review.planeId);
      if (!this.current(review.planeId)) return;
      if (receipt.kind === "refused") {
        this.change = { kind: "none" };
        this.forgetAfter = null;
        this.actionError = receipt.error;
        return;
      }
      if (this.forgetAfter === review.planeId && review.change === "revoke") {
        this.forgetAfter = null;
        this.change = { kind: "none" };
        await this.onForget(review.planeId);
        return;
      }
      this.change = { kind: "done", receipt, review };
      if (receipt.kind === "applied") {
        const fingerprint = formatFingerprint(review.fingerprint);
        this.notice =
          review.change === "revoke"
            ? `${fingerprint} can no longer control ${review.planeLabel}.`
            : review.change === "disable"
              ? `${fingerprint} is disabled on ${review.planeLabel}.`
              : `${fingerprint} is enabled on ${review.planeLabel}.`;
        void this.load();
      }
    } catch (error: unknown) {
      // Uncertain: stop before forgetting; the user can retry or forget only.
      this.forgetAfter = null;
      const uncertain = { kind: "uncertain" as const, review, error: publicError(error) };
      this.retained.set(review.planeId, uncertain);
      if (this.current(review.planeId)) this.change = uncertain;
    }
  }
}
