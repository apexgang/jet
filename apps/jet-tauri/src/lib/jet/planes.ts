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

export type ClientIdentity = {
  clientId: string;
  key: "not_created" | "present" | "session_only" | "unavailable" | "locked" | "unsupported" | "unknown";
  fingerprint: string | null;
};

export type PlaneSelection = { planeId: PlaneId; conversationId: string };

export type PlanesSnapshot = {
  planes: Plane[];
  identity: ClientIdentity;
  restoredSelection: PlaneSelection | null;
  notice: "registry_reset" | null;
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

/** Registry snapshot. Never connects to a Plane. */
export const listPlanes = () => invoke<PlanesSnapshot>("list_planes");

export const loadPlaneDetail = (planeId: PlaneId) =>
  invoke<PlaneDetail>("load_plane_detail", { planeId });

/** The label a Plane is presented with; "This computer" for the local Plane. */
export function planeLabel(snapshot: PlanesSnapshot | null, planeId: PlaneId): string {
  const plane = snapshot?.planes.find((candidate) => candidate.planeId === planeId);
  if (plane) return plane.label;
  return planeId === LOCAL_PLANE ? "This computer" : "Unknown Plane";
}
