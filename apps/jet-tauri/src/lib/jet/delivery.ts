import type { PublicError } from "./bridge";
import { invoke } from "@tauri-apps/api/core";

export type DeliveryOperation =
  | { kind: "branch"; name: string }
  | { kind: "commit"; run_id: string; turn: number }
  | { kind: "push"; remote: string }
  | { kind: "draft_pull_request"; run_id: string; turn: number; remote: string; base: string };

export type Delivery = {
  id: string;
  operation: DeliveryOperation["kind"];
  destination: string;
  checkpoint: string | null;
  status: "pending" | "completed" | "failed" | "outcome_unknown";
  head: string | null;
  branch: string | null;
  pullRequest: string | null;
  code: string | null;
  acknowledged: boolean;
  title: string | null;
};
export type DeliveryReview = {
  reviewId: string;
  conversationId: string;
  workingTree: string;
  operation: DeliveryOperation;
  checkpointFiles: number | null;
  contentComplete: boolean | null;
};
export const loadDeliveries = (conversationId: string) =>
  invoke<Delivery[]>("load_deliveries", { conversationId });
export const prepareDelivery = (conversationId: string, operation: DeliveryOperation) =>
  invoke<DeliveryReview>("prepare_delivery", { conversationId, operation });
export const executeDelivery = (reviewId: string) =>
  invoke<{ kind: "accepted"; deliveryId: string } | { kind: "refused"; error: PublicError }>("execute_delivery", { reviewId });
export const prepareDeliveryAcknowledgement = (conversationId: string, deliveryId: string) =>
  invoke<string>("prepare_delivery_acknowledgement", { conversationId, deliveryId });
