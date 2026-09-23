import { invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";
import type { PlaneId } from "./planes";

/** Why trust-changing Commands are paused on a Plane, if they are. */
export type PairingMutations = {
  allowed: boolean;
  reason: "security_degraded" | "store_read_only" | null;
};

/**
 * How far a Pairing offer has got. The authentication string is never sent to
 * the owner: the owner types the string shown on the other computer.
 */
export type PairingProgress =
  | { kind: "offered" }
  | { kind: "awaiting_confirmation"; clientId: string; clientIsThisComputer: boolean }
  | { kind: "confirmed"; clientId: string }
  | { kind: "ended"; reason: "expired" | "too_many_attempts" | "gate_closed" };

export type PendingPairing = {
  offerId: string;
  method: "manual_code" | "qr_payload";
  progress: PairingProgress;
  attemptsRemaining: number;
  openedAtUnixMs: string;
  expiresAtUnixMs: string;
};

export type PairedClient = {
  clientId: string;
  fingerprint: string;
  access: "enabled" | "disabled";
  pairedAtUnixMs: string;
  pairingProtocol: string;
  isThisComputer: boolean;
};

export type PairingView = {
  planeId: PlaneId;
  cursor: string;
  gate: "open" | "closed";
  pending: PendingPairing | null;
  clients: PairedClient[];
  thisClientId: string;
  mutations: PairingMutations;
};

/** The one-time code, disclosed once. `code` is null when it was already disclosed. */
export type OfferDisclosure = {
  offerId: string;
  code: string | null;
  alreadyDisclosed: boolean;
  expiresAtUnixMs: string;
  attemptsRemaining: number;
};

export type ClientChange = "enable" | "disable" | "revoke";

export type ClientChangeReview = {
  reviewId: string;
  planeId: PlaneId;
  planeLabel: string;
  planeIdentity: string | null;
  clientId: string;
  fingerprint: string;
  change: ClientChange;
  isThisComputer: boolean;
  viaThisPlaneConnection: boolean;
};

export type ClientChangeReceipt =
  | { kind: "applied"; client: PairedClient | null; revoked: boolean }
  | { kind: "applied_unverified"; change: "disable" | "revoke" }
  | { kind: "refused"; error: PublicError };

/** Reads Pairing on a Plane and refreshes that Plane's health. */
export const loadPairing = (planeId: PlaneId) => invoke<PairingView>("load_pairing", { planeId });

/** Opens or closes a Plane's Pairing gate and returns the re-read Pairing. */
export const setPairingGate = (planeId: PlaneId, gate: "open" | "closed") =>
  invoke<PairingView>("set_pairing_gate", { planeId, gate });

/** Opens one manual-code offer. The code is shown once and never again. */
export const openPairingOffer = (planeId: PlaneId) =>
  invoke<OfferDisclosure>("open_pairing_offer", { planeId });

/** Confirms a claim with the string typed from the other computer. */
export const confirmPairingRequest = (planeId: PlaneId, offerId: string, authenticationString: string) =>
  invoke<PendingPairing>("confirm_pairing_request", { planeId, offerId, authenticationString });

/** Reads Pairing fresh and stores a native, Plane-bound review of one change. */
export const preparePairedClientChange = (planeId: PlaneId, clientId: string, change: ClientChange) =>
  invoke<ClientChangeReview>("prepare_paired_client_change", { planeId, clientId, change });

/** Executes a review on the Plane it was prepared against, never another. */
export const executePairedClientChange = (reviewId: string) =>
  invoke<ClientChangeReceipt>("execute_paired_client_change", { reviewId });
