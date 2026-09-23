import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import {
  addRemotePlane,
  cancelRemotePairing,
  claimRemotePairing,
  completeRemotePairing,
  repairRemotePlane,
  type AddPlaneResult,
  type Enrollment,
  type Plane,
  type PlaneId,
} from "$lib/jet/planes";

import { offerLapsed, secureStorageProblem } from "./model";

/**
 * The Add a Plane / Pair again wizard. `repairOf` is the Plane being paired
 * again in place; its destination is fixed.
 */
export type EnrollmentState =
  | { step: "closed" }
  | { step: "destination"; value: string; error: PublicError | null }
  | { step: "checking"; destination: string; repairOf: PlaneId | null }
  | { step: "secure_storage"; destination: string; repairOf: PlaneId | null; error: PublicError }
  | {
      step: "code";
      draftId: string;
      destination: string;
      repairOf: PlaneId | null;
      value: string;
      error: PublicError | null;
    }
  | { step: "claiming"; draftId: string; destination: string; repairOf: PlaneId | null }
  | {
      step: "confirm";
      draftId: string;
      repairOf: PlaneId | null;
      enrollment: Enrollment;
      error: PublicError | null;
    }
  | { step: "completing"; draftId: string; repairOf: PlaneId | null; enrollment: Enrollment }
  | { step: "done"; plane: Plane; repaired: boolean }
  | {
      step: "failed";
      error: PublicError;
      destination: string;
      repairOf: PlaneId | null;
      draftId: string | null;
      retry: "destination" | "code";
    };

/** Refusals of the typed address itself keep the user on the first step. */
const DESTINATION_CODES = new Set([
  "plane.destination_invalid",
  "plane.already_registered",
  "plane.limit_reached",
]);

/** Refusals of the typed code keep the user on the code step. */
const CODE_CODES = new Set([
  "enrollment.code_invalid",
  "pairing.secret_rejected",
  "pairing.none_offered",
  "pairing.gate_closed",
]);

/**
 * Drives enrollment through the native commands. Every async step captures
 * a generation; Cancel and Close bump it, so a late reply never moves the
 * wizard, and a late claim's ticket is cancelled natively.
 */
export class PlaneEnrollment {
  state = $state<EnrollmentState>({ step: "closed" });

  private generation = 0;
  private readonly paired: (plane: Plane, repaired: boolean) => Promise<void> | void;

  constructor(paired: (plane: Plane, repaired: boolean) => Promise<void> | void) {
    this.paired = paired;
  }

  get open(): boolean {
    return this.state.step !== "closed";
  }

  /** The native draft or ticket this wizard holds, if any. */
  private get nativeId(): string | null {
    switch (this.state.step) {
      case "code":
      case "claiming":
        return this.state.draftId;
      case "confirm":
      case "completing":
        return this.state.enrollment.ticketId;
      case "failed":
        return this.state.draftId;
      default:
        return null;
    }
  }

  /** Add a Plane: starts at the SSH address. */
  startAdd(): void {
    this.cancel();
    this.state = { step: "destination", value: "", error: null };
  }

  /** Pair again: starts at checking, with the destination fixed. */
  startRepair(planeId: PlaneId, destination: string): Promise<void> {
    this.cancel();
    return this.check(destination, planeId, false);
  }

  setDestination(value: string): void {
    if (this.state.step === "destination") this.state = { ...this.state, value, error: null };
  }

  setCode(value: string): void {
    if (this.state.step === "code") this.state = { ...this.state, value, error: null };
  }

  submitDestination(): Promise<void> {
    if (this.state.step !== "destination") return Promise.resolve();
    return this.check(this.state.value.trim(), null, false);
  }

  /** Secure storage step: run the ADR-0076 probe again. */
  checkAgain(): Promise<void> {
    if (this.state.step !== "secure_storage") return Promise.resolve();
    return this.check(this.state.destination, this.state.repairOf, false);
  }

  /** Secure storage unavailable: pair with a key kept in memory only. */
  pairForSessionOnly(): Promise<void> {
    if (this.state.step !== "secure_storage") return Promise.resolve();
    if (this.state.error.code !== "identity.secret_store_unavailable") return Promise.resolve();
    return this.check(this.state.destination, this.state.repairOf, true);
  }

  private async check(destination: string, repairOf: PlaneId | null, sessionOnly: boolean): Promise<void> {
    const generation = ++this.generation;
    this.state = { step: "checking", destination, repairOf };
    let result: AddPlaneResult;
    try {
      result = repairOf
        ? await repairRemotePlane(repairOf, sessionOnly)
        : await addRemotePlane(destination, sessionOnly);
    } catch (error: unknown) {
      if (generation !== this.generation) return;
      const failure = publicError(error);
      if (secureStorageProblem(failure)) {
        this.state = { step: "secure_storage", destination, repairOf, error: failure };
      } else if (!repairOf && DESTINATION_CODES.has(failure.code)) {
        this.state = { step: "destination", value: destination, error: failure };
      } else {
        this.state = { step: "failed", error: failure, destination, repairOf, draftId: null, retry: "destination" };
      }
      return;
    }
    if (generation !== this.generation) {
      if (result.kind === "pairing_required") void cancelRemotePairing(result.draftId).catch(() => undefined);
      // Cancelled too late: the Plane is registered natively all the same.
      else this.pairedAfterCancel(result.plane, repairOf !== null);
      return;
    }
    if (result.kind === "connected") {
      await this.finished(result.plane, repairOf !== null);
      return;
    }
    this.state = {
      step: "code",
      draftId: result.draftId,
      destination: result.destination,
      repairOf,
      value: "",
      error: null,
    };
  }

  async submitCode(): Promise<void> {
    if (this.state.step !== "code") return;
    const { draftId, destination, repairOf, value } = this.state;
    const generation = ++this.generation;
    this.state = { step: "claiming", draftId, destination, repairOf };
    try {
      const enrollment = await claimRemotePairing(draftId, value);
      if (generation !== this.generation) {
        // Cancelled meanwhile: the late ticket must not outlive the wizard.
        void cancelRemotePairing(enrollment.ticketId).catch(() => undefined);
        return;
      }
      this.state = { step: "confirm", draftId, repairOf, enrollment, error: null };
    } catch (error: unknown) {
      if (generation !== this.generation) return;
      const failure = publicError(error);
      if (CODE_CODES.has(failure.code) || offerLapsed(failure)) {
        this.state = { step: "code", draftId, destination, repairOf, value: "", error: failure };
      } else if (failure.code === "enrollment.draft_expired") {
        this.state = { step: "failed", error: failure, destination, repairOf, draftId: null, retry: "destination" };
      } else {
        this.state = { step: "failed", error: failure, destination, repairOf, draftId, retry: "code" };
      }
    }
  }

  /**
   * Finish pairing. The shell never ends this step on the countdown: the
   * Plane's own windows decide. Not confirmed yet stays here.
   */
  async finish(): Promise<void> {
    if (this.state.step !== "confirm") return;
    const { draftId, repairOf, enrollment } = this.state;
    const generation = ++this.generation;
    this.state = { step: "completing", draftId, repairOf, enrollment };
    try {
      const plane = await completeRemotePairing(enrollment.ticketId);
      if (generation !== this.generation) {
        // Cancel cannot stop a Finish already running natively.
        this.pairedAfterCancel(plane, repairOf !== null);
        return;
      }
      await this.finished(plane, repairOf !== null);
    } catch (error: unknown) {
      if (generation !== this.generation) return;
      const failure = publicError(error);
      if (offerLapsed(failure)) {
        this.state = {
          step: "code",
          draftId,
          destination: enrollment.destination,
          repairOf,
          value: "",
          error: failure,
        };
      } else if (failure.code === "pairing.not_confirmed" || failure.retryable) {
        // Not confirmed yet, or uncertain: Finish again (same request natively).
        this.state = { step: "confirm", draftId, repairOf, enrollment, error: failure };
      } else {
        this.state = {
          step: "failed",
          error: failure,
          destination: enrollment.destination,
          repairOf,
          draftId,
          retry: "code",
        };
      }
    }
  }

  /**
   * A cancelled wizard whose native step still registered the Plane: the
   * Planes list, Recent and the feed catch up, but the wizard stays closed.
   */
  private pairedAfterCancel(plane: Plane, repaired: boolean): void {
    void Promise.resolve()
      .then(() => this.paired(plane, repaired))
      .catch(() => undefined);
  }

  private async finished(plane: Plane, repaired: boolean): Promise<void> {
    this.state = { step: "done", plane, repaired };
    await this.paired(plane, repaired);
  }

  /** From a failure: back to the step that can fix it. */
  retry(): Promise<void> {
    if (this.state.step !== "failed") return Promise.resolve();
    const { destination, repairOf, draftId, retry } = this.state;
    if (retry === "code" && draftId) {
      this.state = { step: "code", draftId, destination, repairOf, value: "", error: null };
      return Promise.resolve();
    }
    if (repairOf) return this.check(destination, repairOf, false);
    this.state = { step: "destination", value: destination, error: null };
    return Promise.resolve();
  }

  /** Cancel or close: drops native authority; nothing is sent to the Plane. */
  cancel(): void {
    const id = this.nativeId;
    this.generation += 1;
    this.state = { step: "closed" };
    if (id) void cancelRemotePairing(id).catch(() => undefined);
  }
}
