import { executeDelivery, loadDeliveries, prepareDelivery, prepareDeliveryAcknowledgement, type Delivery, type DeliveryOperation, type DeliveryReview } from "$lib/jet/delivery";
import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";

export type ReviewState =
  | { kind: "ready"; review: DeliveryReview }
  | { kind: "acknowledge"; reviewId: string; deliveryId: string }
  | { kind: "sending"; reviewId: string }
  | { kind: "uncertain"; reviewId: string; error: PublicError }
  | { kind: "queued"; deliveryId: string }
  | { kind: "acknowledged"; deliveryId: string };

export class DeliverySession {
  conversationId = $state<string | null>(null);
  planeId = $state<PlaneId>(LOCAL_PLANE);
  rows = $state<Delivery[]>([]);
  fresh = $state(false);
  loading = $state(false);
  busy = $state(false);
  error = $state<PublicError | null>(null);
  historyError = $state<PublicError | null>(null);
  review = $state<ReviewState | null>(null);
  private generation = 0;
  private loadGeneration = 0;
  private retained = new Map<string, ReviewState>();
  private acknowledgements = new Set<string>();

  /** Selects a Conversation on one Plane. The same UUID on another Plane is another scope. */
  select(id: string | null, planeId: PlaneId = LOCAL_PLANE): void {
    if (id === this.conversationId && planeId === this.planeId) return;
    this.generation++;
    this.loadGeneration++;
    this.conversationId = id;
    this.planeId = planeId;
    this.rows = [];
    this.fresh = false;
    this.loading = false;
    this.busy = false;
    this.error = null;
    this.historyError = null;
    this.review = id ? this.retained.get(scopeKey(planeId, id)) ?? null : null;
  }
  offline(): void { this.loadGeneration++; this.loading = false; this.fresh = false; }
  private remember(planeId: PlaneId, id: string, review: ReviewState | null): void {
    const key = scopeKey(planeId, id);
    if (review) this.retained.set(key, review); else this.retained.delete(key);
    if (this.isSelected(planeId, id)) this.review = review;
  }
  private isSelected(planeId: PlaneId, id: string): boolean {
    return this.conversationId === id && this.planeId === planeId;
  }
  async refresh(): Promise<void> {
    const id = this.conversationId;
    const planeId = this.planeId;
    if (!id) return;
    const request = ++this.loadGeneration;
    this.loading = true;
    try {
      const rows = await loadDeliveries(id, planeId);
      if (!this.isSelected(planeId, id) || request !== this.loadGeneration) return;
      this.rows = rows;
      this.fresh = true;
      this.historyError = null;
    } catch (error: unknown) {
      if (!this.isSelected(planeId, id) || request !== this.loadGeneration) return;
      this.fresh = false;
      this.historyError = publicError(error);
    } finally {
      if (this.isSelected(planeId, id) && request === this.loadGeneration) this.loading = false;
    }
  }
  async prepare(operation: DeliveryOperation): Promise<void> {
    const id = this.conversationId;
    const planeId = this.planeId;
    if (!id || this.busy || !this.fresh || this.review?.kind === "uncertain") return;
    const generation = this.generation;
    this.busy = true;
    this.error = null;
    try {
      const review = await prepareDelivery(id, operation, planeId);
      if (this.generation === generation) this.remember(planeId, id, { kind: "ready", review });
    } catch (error: unknown) {
      if (this.generation === generation) this.error = publicError(error);
    } finally { if (this.generation === generation) this.busy = false; }
  }
  async acknowledge(deliveryId: string): Promise<void> {
    const id = this.conversationId;
    const planeId = this.planeId;
    if (!id || this.busy || !this.fresh || this.review?.kind === "uncertain") return;
    const generation = this.generation;
    this.busy = true;
    this.error = null;
    try {
      const reviewId = await prepareDeliveryAcknowledgement(id, deliveryId, planeId);
      if (this.generation !== generation) return;
      this.acknowledgements.add(reviewId);
      this.remember(planeId, id, { kind: "acknowledge", reviewId, deliveryId });
    } catch (error: unknown) {
      if (this.generation === generation) this.error = publicError(error);
    } finally { if (this.generation === generation) this.busy = false; }
  }
  cancel(): void {
    if (this.conversationId && (this.review?.kind === "ready" || this.review?.kind === "acknowledge")) this.remember(this.planeId, this.conversationId, null);
  }
  async confirm(): Promise<void> {
    const id = this.conversationId;
    const planeId = this.planeId;
    const review = this.review;
    if (!id || !review || this.busy || !["ready", "acknowledge", "uncertain"].includes(review.kind)) return;
    const reviewId = review.kind === "ready" ? review.review.reviewId : "reviewId" in review ? review.reviewId : null;
    if (!reviewId) return;
    const generation = this.generation;
    this.busy = true;
    this.error = null;
    this.remember(planeId, id, { kind: "sending", reviewId });
    try {
      const receipt = await executeDelivery(reviewId);
      if (receipt.kind === "refused") {
        this.remember(planeId, id, null);
        if (this.generation === generation) this.error = receipt.error;
        return;
      }
      const deliveryId = receipt.deliveryId;
      this.remember(planeId, id, { kind: this.acknowledgements.has(reviewId) ? "acknowledged" : "queued", deliveryId });
      if (this.generation === generation) await this.refresh();
    } catch (error: unknown) {
      const failure = publicError(error);
      // Only a typed refusal proves the daemon rejected admission. An IPC,
      // transport or decoding failure keeps the exact native request token.
      this.remember(planeId, id, { kind: "uncertain", reviewId, error: failure });
      if (this.generation === generation) this.error = failure;
    } finally { if (this.generation === generation) this.busy = false; }
  }
}

function scopeKey(planeId: PlaneId, conversationId: string): string {
  return `${planeId}\u0000${conversationId}`;
}
