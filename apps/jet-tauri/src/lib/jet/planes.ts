import { invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";

/** Opaque native Plane handle: "local" or a native-issued UUID. */
export type PlaneId = string;

export const LOCAL_PLANE: PlaneId = "local";

export type PlaneConnection =
  | { state: "idle" }
  | { state: "connecting" }
  | { state: "online" }
  | { state: "reconnecting"; error: PublicError }
  | { state: "failed"; error: PublicError };

export type FeatureName =
  | "remote_login"
  | "pairing"
  | "capabilities"
  | "conversation_pages"
  | "projects"
  | "workspaces"
  | "search"
  | "run_supervision"
  | "workspace_terminals"
  | "approval_retry"
  | "git_delivery";

export type FeatureSupport = {
  feature: FeatureName;
  requiredMinor: number;
  support: "supported" | "unsupported" | "unknown";
};

export type PlaneHealth = {
  security: "trusted" | "degraded" | "unknown";
  store: "serving" | "read_only" | "unknown";
  /**
   * The Deletion ledger a snapshot restore depends on. `unsupported` when the
   * Plane reports Recovery on a minor that does not name the ledger;
   * `unknown` before a status or without a Recovery section.
   */
  ledger: "verified" | "corrupt" | "unsupported" | "unknown";
};

/** What the shell can prove about the negotiated minor. Feature table only. */
export type ProtocolKnowledge = {
  exact: number | null;
  atLeast: number;
  atMost: number | null;
};

export type Plane = {
  planeId: PlaneId;
  kind: "local" | "remote";
  label: string;
  planeIdentity: string | null;
  connection: PlaneConnection;
  coreVersion: string | null;
  credential: "durable" | "session" | null;
  security: PlaneHealth["security"];
  store: PlaneHealth["store"];
  features: FeatureSupport[];
  protocol: ProtocolKnowledge;
};

/** What happened at launch to an unreadable stored client identity. */
export type IdentityNotice = "identity_recovered" | "identity_replaced" | "identity_unsaved";

export type ClientIdentity = {
  clientId: string;
  key: "not_created" | "present" | "session_only" | "unavailable" | "locked" | "unsupported" | "unknown";
  fingerprint: string | null;
  notice: IdentityNotice | null;
};

/**
 * Why the saved Planes did not load as stored: set aside and reset, or kept
 * unchanged and read-only because a newer Jet wrote them or they couldn't be
 * read this time.
 */
export type RegistryNotice = "registry_reset" | "registry_newer" | "registry_unreadable";

export type PlaneSelection = { planeId: PlaneId; conversationId: string };

export type PlanesSnapshot = {
  planes: Plane[];
  identity: ClientIdentity;
  restoredSelection: PlaneSelection | null;
  notice: RegistryNotice | null;
  maximumRemotePlanes: number;
};

export type PlaneDetail = {
  plane: Plane;
  platform: string | null;
  harnesses: string[];
  crafts: Array<[string, string]>;
  degraded: string[];
  missingTools: string[];
  issues: Array<{ section: "connection" | "status" | "capabilities"; error: PublicError }>;
};

/** `connected`: the key already worked. Otherwise pairing needs the code. */
export type AddPlaneResult =
  | { kind: "connected"; plane: Plane }
  | { kind: "pairing_required"; draftId: string; destination: string };

/**
 * The confirm step. `authenticationString` was computed on this computer
 * from the validated transcript; it is never the Plane's own copy.
 */
export type Enrollment = {
  ticketId: string;
  destination: string;
  planeIdentity: string;
  authenticationString: string;
  confirmByUnixMs: string;
  sessionOnly: boolean;
};

/** Registry snapshot. Never connects to a Plane. */
export const listPlanes = () => invoke<PlanesSnapshot>("list_planes");

export const loadPlaneDetail = (planeId: PlaneId) =>
  invoke<PlaneDetail>("load_plane_detail", { planeId });

/** Pairs this computer with a remote Plane reached over SSH, or logs in. */
export const addRemotePlane = (destination: string, sessionOnly = false) =>
  invoke<AddPlaneResult>("add_remote_plane", { destination, sessionOnly });

/** Pair again in place: same Plane handle, selection and feeds. */
export const repairRemotePlane = (planeId: PlaneId, sessionOnly = false) =>
  invoke<AddPlaneResult>("repair_remote_plane", { planeId, sessionOnly });

export const claimRemotePairing = (draftId: string, code: string) =>
  invoke<Enrollment>("claim_remote_pairing", { draftId, code });

export const completeRemotePairing = (ticketId: string) =>
  invoke<Plane>("complete_remote_pairing", { ticketId });

/** Drops the native draft or ticket. Nothing is sent to the Plane. */
export const cancelRemotePairing = (id: string) => invoke<void>("cancel_remote_pairing", { id });

/** Removes a remote Plane from this computer only. */
export const forgetRemotePlane = (planeId: PlaneId) =>
  invoke<PlanesSnapshot>("forget_remote_plane", { planeId });

/** The label a Plane is presented with; "This computer" for the local Plane. */
export function planeLabel(snapshot: PlanesSnapshot | null, planeId: PlaneId): string {
  const plane = snapshot?.planes.find((candidate) => candidate.planeId === planeId);
  if (plane) return plane.label;
  return planeId === LOCAL_PLANE ? "This computer" : "Unknown Plane";
}
